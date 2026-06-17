# System Proxy Reliability and Diagnostics

## 状态

已收敛为可执行技术方案。本文定义下一轮 system proxy 可靠性、wake 后检查、结构化诊断和现场取证能力的落地规格。

### 实施进度（截至 2026-06-17）

已落地：

- `bifrost-power::power_notifications::PowerNotificationWatcher` 已实现 macOS IOKit power notification 监听（`crates/bifrost-power/src/power_notifications.rs`）。
- `bifrost-cli` 已新增 `system-proxy lifecycle-helper` 子命令（`crates/bifrost-cli/src/commands/system_proxy.rs::run_system_proxy_lifecycle_helper`），覆盖 parent-pid + start-time 监控、SIGTERM/SIGINT/SIGHUP 处理、`PowerEvent::SystemHasPoweredOn` 后调用 `reconcile_system_proxy_after_power_wake` 与 `cleanup_or_restart_managed_runtime`。
- `bifrost-admin` 在 enable/disable 路径上接入 lifecycle helper（`crates/bifrost-admin/src/state.rs::SystemProxyLifecycleHelperState` 与 `crates/bifrost-admin/src/handlers/proxy.rs` 的 start/stop helper）。
- `bifrost-core::SystemProxyManager` 已提供 `managed_target_has_live_listener` / `last_runtime_target_has_live_listener` / `recover_from_crash` 等 guarded restore 基础能力。
- `bifrost-core::system_proxy_recovery` 提供 `is_retryable_recovery_error` / `is_network_services_not_ready_error` / `retry_with_policy`，对应 network services 未 ready 的 retry 安全网。
- LaunchDaemon one-shot cleanup（`system_proxy_launchd`）保留并继续走 startup recovery。

以下为本文规划但尚未落地（planned, not yet shipped as of 2026-06-17）：

- `bifrost-core::system_proxy_owner_state` 模块与 `system_proxy_owner_state.json` 文件（含 `runtime_start_mode` / `restartable_runtime` / `helper_pid` / `helper_last_heartbeat_at` / `wake_watcher_status` / `last_*` 字段）。
- `bifrost-core::system_proxy_events` 模块、`logs/system_proxy_events.jsonl` 结构化事件、`.system_proxy_diagnostics.lock` 独立诊断锁、10 MiB rotation。
- 抽出的共享决策函数 `reconcile_system_proxy_after_wake(trigger, ...)` 与 `WakeReconcileOutcome` 枚举（目前 wake 路径以 `reconcile_system_proxy_after_power_wake` + `cleanup_or_restart_managed_runtime` 内联实现，未提供具名共享 API）。
- `bifrost-core::managed_runtime_restart::restart_managed_runtime_before_restore` 显式 API 与 `RuntimeRestartOutcome` 枚举。
- helper 5 秒 heartbeat 写入、`helper_heartbeat_stale` / `helper_heartbeat_recovered` 事件、主进程 watchdog 基于 heartbeat 的检查。
- `bifrost status` 系统代理诊断摘要新增字段（lifecycle helper、wake watcher、last reconcile/cleanup、ManagedRuntimeDead 等）。
- WebUI Settings/StatusBar 风险诊断模型。
- `bifrost doctor system-proxy` 命令、`collection_manifest.json` 与 diagnostic bundle。
- Request error aggregation 中的 `network_stack_unready_summary` 5 分钟聚合窗口。
- Windows 平台上的 owner state / event log / parent-death restart-before-restore 端到端验证矩阵。

本节列出的落地清单只代表 wake/lifecycle-helper 链路；以下章节内容仍为目标规格，落地以本节为准。

## 背景

Bifrost 启用系统代理后，如果主进程异常退出、macOS 合盖/休眠唤醒后网络服务暂不可读，或者 cleanup 路径在网络栈未 ready 时过早退出，系统代理可能继续指向已经不可达的 Bifrost listener。用户侧表现是“整个系统没有网络”。

现有 `design/system-proxy.md` 已覆盖 ownership、crash recovery、lifecycle helper、LaunchDaemon one-shot cleanup、runtime target fallback、PID + start_time 身份判定和 macOS network services readiness retry。本文在现有能力上补齐四个确定能力：

1. helper 内 IOKit sleep/wake watcher，唤醒后主动检查系统代理。
2. owner state 和 lifecycle event log，持久记录谁在托管、谁做过恢复、为什么跳过。
3. `status` / WebUI 的风险诊断，直接提示 listener dead、helper missing、external owner 等状态。
4. `doctor system-proxy` 诊断包，一次性收集现场并输出摘要。

## 最终决策

| 决策 | 结论 |
| --- | --- |
| wake notification 放置位置 | 放在 lifecycle helper 内，不放在主进程内 |
| 主进程角色 | 保留 shutdown/listener-exit restore、wake-gap reconcile 兜底、status/WebUI 展示 |
| helper 角色 | parent-death cleanup、IOKit wake watcher、helper heartbeat、wake 后 guarded reconcile |
| LaunchDaemon 角色 | 继续 one-shot，只负责 boot/bootstrap/kickstart 后的残留 cleanup |
| restore 安全边界 | 只恢复或关闭明确匹配 Bifrost managed target / last runtime target 的系统代理 |
| wake 后 listener alive 但网络未 ready | 记录 `network_stack_unready_summary`，不恢复系统代理 |
| watcher 失败 | 不停止 helper，降级到主进程 wake-gap、LaunchDaemon 和 startup recovery |
| 诊断数据 | 写入 `system_proxy_owner_state.json` 和 `logs/system_proxy_events.jsonl` |
| 诊断状态并发 | owner state 和 event log 使用独立诊断锁与原子写，不复用 system proxy 写锁 |
| 心跳落点 | helper heartbeat 只更新 owner state，不按 5 秒频率写 JSONL |
| 主进程已死但代理残留 | 对 daemon/desktop 等可托管运行时优先尝试自动重启主进程；重启失败或不可托管时才 guarded restore/disable |
| Windows 边界 | Windows 落地跨平台诊断、helper parent-death cleanup、可托管 runtime restart-before-restore、status/doctor；不实现 IOKit watcher、LaunchDaemon、networksetup readiness |

## 目标

- 系统代理由 Bifrost 托管时，能从落盘状态回答 owner、runtime、helper、wake watcher、launchd、最近 cleanup/reconcile 结果。
- macOS sleep/wake 后，运行中的 helper 能收到 IOKit power notification，并触发一次受 ownership 保护的 system proxy 检查。
- Bifrost listener dead 且系统代理仍指向 Bifrost target 时，尽快 guarded restore。
- 如果 listener dead 但上次 runtime 是可托管 daemon/desktop 模式，先尝试自动重启主进程；只有重启失败、超时或不允许重启时才恢复/关闭系统代理。
- Bifrost listener alive 但网络栈未 ready 时，不误恢复、不破坏当前代理。
- 用户反馈类似问题时，可以通过 `doctor system-proxy` 获取可诊断 bundle，而不是人工拼多天日志。
- 不误伤 Surge、Clash、VPN 或用户手动配置的外部代理。

## 非目标

- 不阻止 macOS sleep。合盖属于 forced sleep，应用只能收到通知并尽快 ack。
- 不接管所有网络可用性问题。上游网络、DNS、Wi-Fi 未 ready 不等同于 Bifrost listener 故障。
- 不改变 HTTP/HTTPS/SOCKS 转发、规则匹配、TLS 解包逻辑。
- 不把 LaunchDaemon `loaded` 当成 cleanup-daemon 常驻进程。one-shot 退出是正常状态。
- 不在前台 CLI 调试进程异常退出后静默拉起新的主进程；前台模式默认只做恢复/关闭系统代理，避免违背用户显式停止或调试意图。

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
- helper 和主进程共用同一套 wake reconcile 决策函数，只是 trigger 不同。
- 诊断状态写入不能长时间持有 `.system_proxy.lock`，避免 heartbeat/event 写入阻塞系统代理恢复。

## 平台能力矩阵

| 能力 | macOS | Windows | Linux/其他 |
| --- | --- | --- | --- |
| 系统代理读写 | 支持，逐 network service 写 Web/Secure Web proxy | 支持，沿用 HKCU Internet Settings / WinINET 语义 | 不支持 |
| ownership / crash recovery | 支持 | 支持 | 不启用 |
| lifecycle helper | 支持 | 支持，继续使用独立进程组 / detached process | 不启动 |
| helper parent-death cleanup | 支持 | 支持 | 不支持 |
| helper heartbeat | 支持 | 支持 | 不支持 |
| 可托管运行时自动重启 | 支持，覆盖 parent-death 与 wake dead-listener 场景 | 支持 parent-death 场景；不依赖 macOS wake notification | 不支持 |
| owner state | 支持 | 支持 | 可读但不写 active managed state |
| lifecycle event log | 支持 | 支持 | 可写 unsupported/no-op 事件 |
| IOKit wake watcher | 支持 | 不支持，`wake_watcher_status=unsupported` | 不支持，`wake_watcher_status=unsupported` |
| scheduler wake-gap reconcile | 支持 | 本方案不新增 Windows wake-gap 行为 | 不支持 |
| LaunchDaemon one-shot cleanup | 支持 | 不支持 | 不支持 |
| macOS network services readiness retry | 支持 | 不适用 | 不适用 |
| status 诊断 | 完整 helper/watcher/launchd/networksetup 诊断 | helper/system proxy 诊断；watcher 显示 unsupported；launchd 显示 not applicable | system proxy unsupported |
| doctor system-proxy | 完整 bundle | Windows bundle，跳过 macOS-only 项 | unsupported summary |

Windows 非回归约束：

- 不在 Windows 上引入 IOKit、CFRunLoop、LaunchDaemon、`networksetup`、macOS unified log 依赖。
- 不改变 Windows 当前系统代理写入路径；仍沿用现有 `SystemProxyManager` Windows implementation。
- Windows helper 只负责 parent-death cleanup、可托管 runtime restart-before-restore、heartbeat、owner state/event 写入，不做 sleep/wake watcher。
- Windows `wake_watcher_status=unsupported` 是正常状态，不触发 warning，不触发 helper restart。
- Windows `doctor system-proxy` 必须跳过 macOS-only 采集项，并在 `collection_manifest.json` 中标记 `not_applicable`。
- Windows E2E 验证 helper parent-death cleanup、可托管 runtime restart-before-restore、owner state/event/status/doctor；不验证 TC-CSP-31/TC-CSP-32 的 macOS sleep/wake 行为。

## 新增与调整模块

### `bifrost-power::power_notifications`

新增 macOS-only IOKit power notification 封装。非 macOS 返回 unsupported。

职责：

- 封装 IOKit FFI：
  - `IORegisterForSystemPower`
  - `IODeregisterForSystemPower`
  - `IONotificationPortGetRunLoopSource`
  - `IONotificationPortDestroy`
  - `CFRunLoopAddSource`
  - `CFRunLoopRemoveSource`
  - `CFRunLoopRun`
  - `CFRunLoopStop`
  - `IOAllowPowerChange`
- 提供：
  ```text
  spawn_power_event_watcher(sender) -> Result<PowerWatcherHandle>
  ```
- 在独立线程运行 CFRunLoop，线程名：
  ```text
  bifrost-system-proxy-power-events
  ```
- callback 只做两件事：
  - 对需要 ack 的 sleep message 调用 `IOAllowPowerChange`。
  - 把 power event 投递到 channel。

callback 禁止直接执行：

- `networksetup`
- listener probe
- `SystemProxyManager::recover_from_crash`
- 文件锁等待

消息处理要求：

- `kIOMessageCanSystemSleep`：立即 `IOAllowPowerChange`，写 `system_can_sleep` event，不阻止 idle sleep。
- `kIOMessageSystemWillSleep`：立即 `IOAllowPowerChange`，写 `system_will_sleep` event。
- `kIOMessageSystemWillPowerOn`：写 `system_will_power_on` event，不执行 reconcile。
- `kIOMessageSystemHasPoweredOn`：写 `system_has_powered_on` event，并由 worker 线程触发 debounce 后 reconcile。
- 未识别 message：写 debug event 或 tracing debug，不影响 watcher。

`PowerWatcherHandle` drop 要求：

- 注销 IOKit power notification。
- 从 CFRunLoop 移除 source。
- 调用 `CFRunLoopStop`。
- join watcher thread，超时 2 秒后写 warn 并继续 helper cleanup。

### `bifrost-core::system_proxy_owner_state`

新增 owner state 读写模块，文件：

```text
system_proxy_owner_state.json
```

该文件用于诊断和跨组件状态聚合，不替代 `proxy_state.json` / `proxy_backup.json`。恢复 original proxy 的权威状态仍是现有 managed state。

字段：

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

- CLI/daemon start 写入 runtime 基础信息。
- 前台 `bifrost start` 写 `runtime_start_mode=foreground`、`restartable_runtime=false`。
- `bifrost start --daemon` 和 Desktop/托管启动写 `runtime_start_mode=daemon|desktop`、`restartable_runtime=true`。
- Admin API / WebUI / CLI 启用系统代理成功后写入 `enabled_source` 和 listener target。
- helper 启动后写入 helper pid / started_at_ms。
- helper heartbeat 每 5 秒更新 `helper_last_heartbeat_at`。
- helper 内 watcher 启动、失败、停止、收到 wake event 时更新 `wake_watcher_*`。
- periodic reconcile、scheduler wake-gap、power notification reconcile、shutdown restore、launchd cleanup 更新最近 attempt/result。
- clean shutdown 完成 restore 后写入 `clean_shutdown=true`，并将 active helper/watcher 状态标记为 disabled。

兼容规则：

- 缺失 owner state 不阻止现有 recovery。
- JSON 损坏时写 warn 和 lifecycle event，但不能阻断 `proxy_state.json` / `runtime.json` 的 guarded recovery。

并发与权限规则：

- owner state 使用独立诊断锁：
  ```text
  .system_proxy_diagnostics.lock
  ```
- 该锁只保护 `system_proxy_owner_state.json` 和 `logs/system_proxy_events.jsonl`，不得包住 `networksetup` 调用。
- owner state 写入必须采用 read-merge-atomic-write：
  1. 持有 `.system_proxy_diagnostics.lock`。
  2. 读取当前 JSON；损坏时保留原文件为 `.corrupt.<timestamp>`。
  3. 只更新本次调用负责的字段，避免 heartbeat 覆盖 cleanup result。
  4. 写入临时文件。
  5. `fsync` 临时文件。
  6. `rename` 覆盖正式文件。
- macOS root LaunchDaemon 和普通用户进程都可能写诊断文件；锁文件和日志目录权限必须沿用 `.system_proxy.lock` 的 nofollow + fd chmod 思路，避免 symlink 跟随和 root-owned strict lock。

### `bifrost-core::system_proxy_events`

新增结构化事件日志：

```text
logs/system_proxy_events.jsonl
```

事件基础字段：

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
  "current_proxy": {
    "enabled": true,
    "host": "127.0.0.1",
    "port": 9900,
    "managed_by_bifrost": true
  },
  "expected_proxy": {
    "host": "127.0.0.1",
    "port": 9900
  },
  "listener_alive": true,
  "network_services_readable": true,
  "decision": "keep_managed_proxy",
  "error": null,
  "duration_ms": 42
}
```

必须支持的事件：

- `startup_state_snapshot`
- `startup_unclean_previous_runtime`
- `system_proxy_enable_requested`
- `system_proxy_enable_applied`
- `system_proxy_disable_requested`
- `system_proxy_disable_applied`
- `helper_start_requested`
- `helper_started`
- `helper_heartbeat_stale`
- `helper_heartbeat_recovered`
- `helper_missing`
- `helper_restart_requested`
- `launchd_status_checked`
- `launchd_install_requested`
- `wake_notification_watcher_started`
- `wake_notification_watcher_failed`
- `wake_notification_watcher_stopped`
- `system_can_sleep`
- `system_will_sleep`
- `system_will_power_on`
- `system_has_powered_on`
- `wake_notification_reconcile_started`
- `wake_notification_reconcile_completed`
- `wake_notification_reconcile_skipped_listener_alive`
- `wake_notification_reconcile_restarted_runtime`
- `wake_notification_reconcile_restored_dead_listener`
- `runtime_restart_considered`
- `runtime_restart_skipped`
- `runtime_restart_started`
- `runtime_restart_succeeded`
- `runtime_restart_failed`
- `wake_gap_detected`
- `reconcile_started`
- `reconcile_completed`
- `cleanup_started`
- `cleanup_retrying`
- `cleanup_restored`
- `cleanup_disabled_stale_proxy`
- `cleanup_skipped_external_owner`
- `cleanup_failed`
- `network_stack_unready_summary`

写入要求：

- append-only JSONL。
- 每条事件必须单行写入；写入前先序列化完整 JSON 字符串，再持有 `.system_proxy_diagnostics.lock` append。
- 单条写入失败不影响恢复主流程，但必须在主日志打 warn。
- 不记录 URL path、cookie、Authorization、请求体、rules、values、scripts 内容。
- host/port 可以记录，因为 system proxy 诊断必须知道 target。
- 首版必须实现基础滚动：单文件超过 10 MiB 后 rotate 为 `system_proxy_events.jsonl.1`，最多保留 3 个历史文件。
- doctor bundle 默认只导出 `--since` 时间范围内的事件；如果事件无时间字段或解析失败，将该行放入 `diagnostics/event_parse_errors.jsonl`。
- helper 每 5 秒 heartbeat 只更新 `system_proxy_owner_state.json` 的 `helper_last_heartbeat_at`，不得按心跳频率 append JSONL。
- JSONL 只记录 helper 状态跃迁：started、missing、restart requested、heartbeat stale、heartbeat recovered、watcher failed/stopped 等。

### `bifrost-core::system_proxy_wake_reconcile`

新增共享 wake reconcile 决策函数。该函数只在 macOS wake paths 中启用；Windows 不接入 scheduler gap / power notification wake reconcile，避免把 macOS network services readiness 语义带到 Windows。

```text
reconcile_system_proxy_after_wake(trigger, data_dir, runtime_target) -> WakeReconcileOutcome
```

`trigger` 枚举：

- `power_notification`
- `scheduler_gap`

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

```text
1. 读取 proxy_state.json / proxy_backup.json / runtime.json。
2. 读取当前 macOS per-service Web/Secure Web proxy。
3. 如果 network services 不可读或返回空 service list：
   - 写 Retrying，`reason=network_services_unready`
   - 保留 state
   - 后台 retry
   - 不退回 scutil 聚合状态做 destructive cleanup
4. 如果当前系统代理不指向 Bifrost target：
   - 写 SkippedExternalOwner 或 Unmanaged
   - 不恢复、不关闭
5. 如果当前系统代理指向 Bifrost target：
   - 探测 listener
6. listener alive：
   - 写 KeptManagedProxy
   - 不恢复
7. listener dead：
   - 先进入 restart-before-restore 判断
   - 如果 runtime 可托管且允许自动重启，尝试重启主进程
   - 重启成功且 listener 在超时内恢复，写 RestartedRuntime，不恢复系统代理
   - 重启跳过、失败或超时后，继续执行 guarded restore/disable
8. listener dead 且未能自动重启：
   - 持有 .system_proxy.lock
   - 有 original backup 则 restore original
   - 无 backup 但 target 匹配 last runtime，则 disable stale proxy
   - 写 RestoredDeadListener 或 DisabledStaleProxy
9. 任一可重试错误：
   - 写 Retrying，并在 event 中标记 `retryable=true` 和具体 `reason`
   - 保留 state
10. 任一不可重试错误：
   - 写 Failed，并在 event 中标记 `retryable=false`
   - 保留足够诊断信息
```

### `bifrost-core::managed_runtime_restart`

新增托管运行时重启决策模块，由 wake reconcile 和 parent-death cleanup coordinator 调用。

```text
restart_managed_runtime_before_restore(trigger, data_dir, runtime_snapshot) -> RuntimeRestartOutcome
```

`RuntimeRestartOutcome`：

- `Restarted`
- `SkippedNotRestartable`
- `SkippedCleanStop`
- `SkippedExternalOwner`
- `Failed`
- `TimedOut`

允许自动重启的必要条件：

1. owner state / runtime state 表明 `restartable_runtime=true`。
2. `runtime_start_mode` 为 `daemon` 或 `desktop`；`foreground` 默认不可自动重启。
3. 没有 `clean_shutdown=true`，也没有用户显式 stop/disable 的 clean-stop marker。
4. 当前系统代理仍匹配 Bifrost managed target 或 last runtime target。
5. 旧 pid 不存在，或 pid 存在但 start_time mismatch；如果 pid/start_time 仍匹配，只做 listener 重探测，不启动第二个主进程。
6. `binary_path`、`data_dir`、监听 host/port、必要启动参数可信且来自 Bifrost 写入的 runtime/owner state。
7. helper 或主进程 watchdog 仍能获得 `.system_proxy.lock`，避免 restart 与 restore 并发破坏现场。

重启流程：

1. 写 `runtime_restart_considered`。
2. 如果不满足允许条件，写 `runtime_restart_skipped`，并带上 `reason`。
3. 启动 daemon/desktop 托管主进程，启动参数必须继承上次可信 runtime 配置；不得凭空接管前台 CLI。
4. 写 `runtime_restart_started`，更新 owner state `last_runtime_restart_attempt_at/result=started`。
5. 在 10 秒内轮询 listener 和新 runtime identity：
   - listener alive 且新 pid/start_time 可确认：写 `runtime_restart_succeeded`，outcome=`RestartedRuntime`。
   - 端口仍不可达、启动失败或身份不可信：写 `runtime_restart_failed`，进入 guarded restore/disable fallback。
6. 重启成功后保留系统代理指向 Bifrost，不恢复 original proxy。
7. 重启失败或跳过后，继续执行 `RestoredDeadListener` / `DisabledStaleProxy` 分支，避免用户网络继续指向死端口。

平台边界：

- macOS：wake notification dead-listener、parent-death cleanup、startup recovery 都可以进入 restart-before-restore 判断。
- Windows：不接入 IOKit wake notification；仅在 helper parent-death 或 startup recovery 发现 restartable runtime dead 时可尝试重启。
- Linux/其他：不启用该能力。
- 所有平台都必须先确认 current proxy target 属于 Bifrost；外部代理抢占时禁止自动重启并接管系统代理。

## Helper 内 IOKit Watcher 运行流程

### 启动

`system-proxy lifecycle-helper` 启动后：

1. 读取 data dir。
2. 写 `helper_started` event。
3. 写 owner state 的 helper pid / helper started_at_ms。
4. 如果平台不是 macOS：
   - `wake_watcher_status=unsupported`
   - 不启动 watcher。
5. 如果 macOS 且 runtime config / owner state / managed state 任一表明当前实例正在托管系统代理：
   - 启动 `PowerWatcherHandle`。
   - 成功写 `wake_notification_watcher_started`。
   - 失败写 `wake_notification_watcher_failed`，但 helper 继续执行 parent-death cleanup。
6. 启动 helper heartbeat loop。
7. 启动 parent-death poll loop。
8. 启动 power event worker loop。

重要边界：

- helper 不是新的长期 wake daemon。当前父进程存活时，helper 通过 IOKit watcher 覆盖 sleep/wake；一旦确认父进程已经退出，helper 必须立即执行 guarded cleanup 并退出。
- 如果主进程在睡眠期间已经死亡，helper 在唤醒后可能先收到 power event，也可能先通过 parent-death poll 确认父进程退出。两条路径必须进入同一个 cleanup/reconcile coordinator，最终只执行一次 destructive restore/disable。
- helper 退出后不再接收后续 wake notification；之后的残留恢复仍由 LaunchDaemon one-shot 和 startup recovery 覆盖。

### Power Event Worker

收到 `kIOMessageSystemWillSleep`：

- 写 `system_will_sleep` event。
- 更新 `wake_watcher_last_event_at`。
- 只做轻量 snapshot。
- 不调用 `networksetup`。

收到 `kIOMessageCanSystemSleep`：

- 立即 ack。
- 写 `system_can_sleep` event。
- 不阻止 idle sleep。

收到 `kIOMessageSystemWillPowerOn`：

- 写 `system_will_power_on` event。
- 更新 `wake_watcher_last_event_at`。
- 标记 early wake。
- 不判断 listener，不恢复。

收到 `kIOMessageSystemHasPoweredOn`：

- 写 `system_has_powered_on` event。
- debounce 2 秒。
- 写 `wake_notification_reconcile_started`。
- 调用 `reconcile_system_proxy_after_wake(power_notification, ...)`。
- 写 `wake_notification_reconcile_completed` 和 owner state。

并发协调：

- parent-death cleanup、SIGTERM/SIGINT/SIGHUP cleanup、power notification reconcile 必须通过同一个 helper 内部 coordinator 串行执行。
- coordinator 按事件优先级处理：
  1. explicit signal / 用户 clean stop cleanup：不自动重启主进程，只恢复/关闭 Bifrost 托管代理。
  2. confirmed parent death cleanup：如果 runtime 可托管，先执行 restart-before-restore；否则 guarded restore/disable。
  3. wake notification reconcile：listener dead 时同样先执行 restart-before-restore；listener alive 时 keep。
- 如果 restart-before-restore 已经成功，后续排队的 cleanup 必须观察到新 runtime/listener alive 并跳过 destructive restore。
- 如果 cleanup 已经完成并恢复/关闭了 Bifrost system proxy，后续排队的 wake reconcile 必须观察到 `Unmanaged` 或 `ExternalOwnerSkip` 并安全跳过。
- coordinator 必须写 outcome event，便于区分“wake 后没有动作”是因为 listener alive、external owner，还是因为 parent-death cleanup 已先完成。

### 停止

helper 停止条件：

- Admin API / WebUI / CLI disable 系统代理，且 live state 已确认 clean。
- 主进程正常停止并完成 restore。

停止动作：

- drop `PowerWatcherHandle`。
- `IODeregisterForSystemPower`。
- `IONotificationPortDestroy`。
- 停止 CFRunLoop 线程。
- 等待 watcher 线程退出，最多 2 秒。
- 写 `wake_notification_watcher_stopped`。
- 写 owner state `wake_watcher_status=disabled`。

未收敛场景：

- disable 请求失败。
- networksetup 失败。
- 外部代理抢占导致当前 live state 不 clean。

这些场景不停止 helper，watcher 继续作为安全网。

信号处理：

- clean disable 后主进程可以继续按现有方式停止 helper；helper 收到停止信号时仍应走 guarded cleanup。因为 live state 已 clean，cleanup 应是 no-op 或 `ExternalOwnerSkip`。
- helper 收到 SIGTERM/SIGINT/SIGHUP 时必须先写 `wake_notification_watcher_stopped` 或等价 stopped/cleanup event，再释放 IOKit watcher 资源。
- stopped event 写失败不能阻止 cleanup。

## 主进程 Wake-Gap 兜底

macOS 保留现有 `bifrost-system-proxy-wake-reconcile` 线程。它不再拥有独立决策，只调用共享函数：

```text
reconcile_system_proxy_after_wake(scheduler_gap, ...)
```

触发条件仍是 scheduler gap 超过阈值。

该线程存在的原因：

- watcher 初始化失败时兜底。
- helper 缺失时兜底。
- 某些 macOS 环境 power notification 丢失时兜底。

主进程 wake-gap 线程必须遵守同样的 ownership 和 lock 规则，不能只做盲目 enable。

Windows 不新增 wake-gap reconcile 行为。Windows 的本轮改动只包含 helper heartbeat、owner state/event、status/doctor 诊断和 parent-death cleanup 可观测性。

与 helper watcher 的关系：

- 主进程 wake-gap 是 fallback，不是第二个 owner。
- 当 owner state 显示 helper wake watcher `running` 且最近 wake notification reconcile 已在同一 wake window 内完成，scheduler gap reconcile 可以写 `reconcile_completed` with `decision=skipped_recent_power_notification` 并跳过重复检查。
- wake window 默认 120 秒；该值只用于去重，不用于判断 restore 安全性。

## Helper Heartbeat and Watchdog

helper 每 5 秒写 heartbeat：

```text
helper_last_heartbeat_at
helper_pid
helper_started_at_ms
wake_watcher_status
wake_watcher_last_event_at
wake_watcher_last_reconcile_at
```

主进程 watchdog 检查：

- helper pid 是否存在。
- helper start_time 是否匹配。
- heartbeat 是否超过 20 秒。
- macOS system proxy 托管时，wake watcher 是否为 `running`。

处理：

- helper 存活权威条件是 `pid 存在 && start_time 匹配`。
- 只有以下情况允许重启 helper：
  - pid 不存在，且连续 3 次检查都不可见。
  - pid 存在但 start_time mismatch，说明 PID 复用或 helper 身份不可信。
- heartbeat stale 不触发重启：如果 `pid 存在 && start_time 匹配`，但 `helper_last_heartbeat_at` 超过 20 秒，只写 `helper_heartbeat_stale`，`status` / WebUI 展示 warning，等待下一轮恢复。
- heartbeat 从 stale 恢复时写 `helper_heartbeat_recovered`。
- wake watcher failed 不触发 helper 重启：只展示 warning，并降级到 scheduler wake-gap。
- Windows 上 `wake_watcher_status=unsupported` 不触发 warning，不触发重启，只在 status/doctor 中显示 not applicable。
- 重启 helper 前必须先处理旧 helper：
  - 如果 start_time mismatch 且旧 pid 仍可见，先向旧 pid 发送 stop/terminate，并等待最多 5 秒。
  - 如果旧 helper 未退出，记录 warning；新 helper 仍可启动，但所有 cleanup/reconcile 仍由 `.system_proxy.lock` 串行保护。
- helper restart 成功：写 `helper_restart_requested` / `helper_started`。
- helper restart 失败：`status` 和 WebUI 展示 cleanup helper unavailable。

## Status and WebUI Diagnostics

`bifrost status` 新增系统代理诊断摘要：

```text
System proxy: managed by Bifrost, 127.0.0.1:9900
Listener: alive
Lifecycle helper: alive, last heartbeat 4s ago
Wake watcher: running, last wake event 2m ago
LaunchDaemon cleanup: installed, loaded, one-shot
Last reconcile: success, 2026-06-09 12:03:21
Last cleanup: success, 2026-06-09 12:04:01
```

高风险状态：

```text
System proxy points to Bifrost 127.0.0.1:9900, but listener is not reachable.
Recommended action: bifrost system-proxy restore
```

WebUI Settings/StatusBar 使用同一诊断模型：

| 状态 | UI 行为 |
| --- | --- |
| ManagedActive | 显示 managed、helper healthy、launchd current |
| ExternalOwnerSkip | 显示 occupied by another proxy，开关关闭但允许接管 |
| ManagedRuntimeDead | 红色风险提示，提供 restore 操作入口 |
| HelperMissing | warning，提示 cleanup helper 不可用 |
| WakeWatcherFailed | warning，提示已降级到 scheduler wake-gap |
| ManagedNetworkUnready | warning，提示 wake 后网络栈未 ready，不误报 Bifrost down |

## Doctor Diagnostic Bundle

新增命令：

```bash
bifrost doctor system-proxy --since "2026-06-08 00:00" --bundle ./bifrost-system-proxy-diagnostic.zip
```

默认收集：

- `bifrost --version`
- `bifrost status`
- `bifrost system-proxy status`
- `bifrost system-proxy launchd status`
- `scutil --proxy`
- `networksetup -listallnetworkservices`
- `networksetup -getwebproxy <service>`
- `networksetup -getsecurewebproxy <service>`
- `runtime.json`
- `bifrost.pid`
- `proxy_state.json`
- `proxy_backup.json`
- `system_proxy_owner_state.json`
- `logs/system_proxy_events.jsonl`
- `logs/bifrost*.log`
- `/var/log/bifrost-system-proxy-cleanup.log`
- `/var/log/bifrost-system-proxy-cleanup.err`
- 指定时间范围内的 macOS unified log 摘要

Windows bundle 收集：

- `bifrost --version`
- `bifrost status`
- `bifrost system-proxy status`
- Windows 当前代理状态（由 Bifrost 内部 Windows system proxy reader 输出）
- `runtime.json`
- `bifrost.pid`
- `proxy_state.json`
- `proxy_backup.json`
- `system_proxy_owner_state.json`
- `logs/system_proxy_events.jsonl`
- `logs/bifrost*.log`

Windows bundle 不收集：

- `networksetup`
- `scutil --proxy`
- LaunchDaemon status
- `/var/log/bifrost-system-proxy-cleanup.*`
- macOS unified log

收集规则：

- doctor 不要求 sudo。
- 无权限读取的文件必须记录为 `missing_or_permission_denied`，不能导致整个 bundle 失败。
- `/var/log`、macOS unified log、`networksetup` 任一收集失败时，summary 必须标出缺失项。
- bundle 内必须包含 `collection_manifest.json`，记录每个采集项的 `collected|missing|permission_denied|failed` 状态。
- 对平台不适用的采集项必须记录为 `not_applicable`，不是 `missing`。

summary 必须输出：

- previous runtime clean/unclean。
- 当前系统代理 target。
- 当前 target 是否匹配 Bifrost owner。
- listener alive/dead。
- helper alive/missing。
- wake watcher running/failed/unsupported/disabled。
- 最近 sleep/wake event。
- 最近 wake reconcile 结果。
- LaunchDaemon installed/loaded/mode。
- recommended action。

Windows summary 规则：

- `wake_watcher_status=unsupported` 作为正常状态展示。
- LaunchDaemon 显示 `not_applicable`。
- 不出现 macOS network services readiness 相关建议。
- 如果系统代理指向 Bifrost target 但 listener dead，仍可建议执行 Windows 对应的 system proxy restore/disable stale proxy。

macOS unified log predicate：

```text
process == "bifrost"
OR process == "launchd"
OR eventMessage CONTAINS[c] "Wake"
OR eventMessage CONTAINS[c] "Sleep"
OR eventMessage CONTAINS[c] "networksetup"
```

默认脱敏：

- URL path
- query
- token
- cookie
- authorization header
- request body
- 用户 home path 默认替换为 `$HOME`

如果当前系统代理指向外部代理，summary 必须明确标记 external owner，并且不得建议恢复外部代理。

## Request Error Aggregation

请求错误日志保留现状，新增 5 分钟窗口聚合事件：

```json
{
  "event": "network_stack_unready_summary",
  "window_secs": 300,
  "since_wake_event_secs": 42,
  "listener_alive": true,
  "system_proxy_state": "ManagedActive",
  "errors": {
    "ENETUNREACH": 16,
    "EADDRNOTAVAIL": 15,
    "dns_lookup_failed": 9,
    "timeout": 64
  },
  "decision": "do_not_restore_listener_alive"
}
```

聚合只用于诊断和避免误判，不直接触发 restore。restore 只能由 ownership + listener dead 决策触发。

跨进程 wake phase 来源：

- upstream error 计数在主进程内完成。
- wake notification 在 helper 内产生。
- 主进程通过 `system_proxy_owner_state.json.wake_watcher_last_event_at` 和 `wake_watcher_last_reconcile_at` 判断当前是否处于 wake phase。
- wake phase 默认窗口为最近 120 秒；只用于错误聚合标签，不参与 restore 安全决策。
- owner state 缺失或 watcher unsupported/failed 时，主进程只能使用自身 scheduler gap 时间标记 wake phase。
- Windows 不启用 wake phase aggregation；Windows request error aggregation 不读取 `wake_watcher_last_event_at`。

## 安全不变量

- 当前系统代理 target 不匹配 Bifrost managed target 或 last runtime target 时，禁止 restore/disable。
- network services 不可读时，禁止退回 `scutil --proxy` 聚合状态做 destructive cleanup。
- wake notification callback 禁止执行耗时恢复逻辑。
- helper watcher 失败不能停止 helper parent-death cleanup。
- 所有写系统代理路径必须持有 `.system_proxy.lock`。
- 所有 owner state / event log 写入必须持有 `.system_proxy_diagnostics.lock`，并且不得在持有该锁时执行 `networksetup`。
- doctor bundle 默认脱敏，不收集 request body、rules、values、scripts 内容。
- doctor bundle 采集失败必须 best-effort，不能为了读取 `/var/log` 要求用户先 sudo。
- 用户显式禁用 helper / launchd 的环境变量必须继续生效。
- Windows 上任何 macOS-only 功能必须以 `cfg(target_os = "macos")` 或运行时 platform guard 隔离，不能影响 Windows build/test。

## 实施阶段

### Phase 1：结构化状态与事件

- 新增 `system_proxy_owner_state.json` 读写。
- 新增 `logs/system_proxy_events.jsonl` 写入 helper。
- 在现有 enable/disable/restore/recover/helper/launchd 路径补事件。
- `status` 先读取 owner state 并展示 helper/last cleanup 基础信息。

验收：

- 单测覆盖 owner state 兼容和 event JSONL schema。
- TC-CSP-28 通过。

### Phase 2：Helper 内 IOKit Watcher

- 新增 macOS IOKit power notification FFI。
- `lifecycle-helper` 启动 watcher。
- `kIOMessageSystemHasPoweredOn` 后触发 shared wake reconcile。
- watcher 状态写入 owner state 和 event log。
- Windows 在本阶段只写 `wake_watcher_status=unsupported`，不改 Windows helper cleanup 行为。

验收：

- fake/integration 测试模拟 will-power-on / has-powered-on。
- TC-CSP-32 通过。

### Phase 3：共享 Wake Reconcile 和主进程兜底收敛

- 抽出 `reconcile_system_proxy_after_wake`。
- helper power notification 和主进程 scheduler gap 共用该函数。
- listener alive + network unready 不 restore。
- listener dead + target match 时先进入 restart-before-restore；不可重启或重启失败才 restore/disable stale proxy。

验收：

- TC-CSP-31 通过。
- 系统代理 E2E 覆盖 listener dead / listener alive / external owner。

### Phase 4：可托管运行时 Restart-Before-Restore

- 在 runtime/owner state 中记录 `runtime_start_mode` 和 `restartable_runtime`。
- 将 daemon/desktop 托管启动标记为可自动重启，foreground CLI 标记为不可自动重启。
- wake reconcile 和 parent-death cleanup 发现 listener dead 时，先尝试自动重启可托管主进程。
- 重启成功后保持系统代理；重启失败、超时或不可重启时进入 guarded restore/disable fallback。
- Windows 仅覆盖 helper parent-death/startup recovery 场景，不新增 wake notification。

验收：

- TC-CSP-34 通过。
- 单测覆盖 foreground 不自动重启、clean stop 不自动重启、external owner 不自动重启、daemon 重启成功、重启失败后 fallback restore。

### Phase 5：Status/WebUI/Doctor

- `bifrost status` 输出诊断摘要。
- WebUI Settings/StatusBar 展示 helper、watcher、listener、external owner 风险。
- 新增 `doctor system-proxy` 诊断包命令。

验收：

- TC-CSP-29 通过。
- TC-CSP-30 通过。

### Phase 6：Request Error Aggregation

- 新增 5 分钟窗口 upstream error aggregation。
- wake 后 2 分钟内的网络错误标记为 wake phase。
- 写 `network_stack_unready_summary` event。

验收：

- 单测覆盖聚合窗口。
- E2E/fake upstream 覆盖 listener alive 时不 restore。

## 测试矩阵

### 单元测试

- owner state：缺字段、旧 schema、损坏 JSON、并发写。
- lifecycle event：必填字段、append failure、脱敏字段、并发 append 单行完整性、10 MiB rotation。
- wake reconcile：listener alive/dead、external owner、network services unreadable、runtime target fallback。
- managed runtime restart：restartable daemon/desktop、foreground skip、clean stop skip、external owner skip、restart success、restart timeout 后 fallback restore。
- helper coordinator：parent-death cleanup 与 wake notification reconcile 同时到达时只执行一次 restore。
- helper heartbeat/watchdog：fresh/stale/missing、PID + start_time mismatch、restart cooldown。
- watchdog restart decision：pid alive + start_time match + stale heartbeat 不重启；watcher failed 不重启；pid missing 或 start_time mismatch 才重启。
- IOKit watcher：message 映射、has-powered-on 后触发 reconcile、will-power-on 不触发 restore。
- IOKit watcher drop：deregister、stop runloop、join timeout 不阻塞 cleanup。
- doctor summary：从 fixture 状态文件生成正确诊断结论。
- Windows platform guards：`wake_watcher_status=unsupported` 不触发 warning/restart；doctor macOS-only 采集项为 `not_applicable`；Windows build 不链接 IOKit/CoreFoundation。

### E2E 测试

- fake `networksetup` 返回空 service list，确认保留 state 并 retry。
- listener dead + proxy points to Bifrost，确认 restore。
- listener alive + network unreachable，确认只写 `network_stack_unready_summary`。
- helper 被 kill，主进程 watchdog 重启 helper。
- daemon/desktop 托管 runtime 被 kill 且系统代理仍指向 Bifrost，确认优先重启主进程；重启失败时 fallback restore。
- doctor bundle 生成 zip，包含 summary 和脱敏日志。
- Windows：helper parent-death cleanup 后，restartable runtime 优先自动重启；不可重启或失败时系统代理不再指向 Bifrost target；owner state/event/status/doctor 正确展示 watcher unsupported。

### 真实场景测试

更新并执行 `human_tests/cli-system-proxy.md`：

- TC-CSP-28：lifecycle event log。
- TC-CSP-29：`bifrost status` 风险诊断。
- TC-CSP-30：doctor bundle。
- TC-CSP-31：sleep/wake 后 listener alive 但网络栈未 ready 不误恢复。
- TC-CSP-32：helper 内 IOKit wake notification 触发系统代理检查。
- TC-CSP-33：Windows watcher unsupported 且不影响 helper cleanup。
- TC-CSP-34：托管运行时 listener dead 且系统代理残留时优先自动重启主进程。

平台执行要求：

- TC-CSP-28/29/30 需要覆盖 macOS 与 Windows。
- TC-CSP-31/32 仅 macOS 执行。
- TC-CSP-34 需要覆盖 macOS daemon/desktop 托管模式；Windows 覆盖 parent-death/startup recovery 触发，不覆盖 sleep/wake 触发。
- Windows 新增或扩展用例必须放在同一文档中标注 Windows-only 步骤，不能要求 `networksetup`、`pmset` 或 macOS LaunchDaemon。

## 校验要求

- `cargo test -p bifrost-core system_proxy`
- `cargo test -p bifrost-power`
- `cargo test -p bifrost-cli system_proxy`
- `cargo test -p bifrost-admin proxy::tests`
- `bash e2e-tests/tests/test_system_proxy_e2e.sh`
- Windows CI / 本地可用环境必须跑 Windows system proxy helper/status/doctor focused tests。
- `cargo test --workspace --all-features`
- `rust-project-validate`

## 文档更新要求

- `design/system-proxy.md` 保持链接到本文。
- `human_tests/cli-system-proxy.md` 保持 TC-CSP-28 到 TC-CSP-34。
- `human_tests/readme.md` 保持系统代理用例计数。
- 如果 WebUI 状态展示落地，更新对应 Settings/StatusBar human_tests。

## 场景化路径推演

本节把事故现场按触发源和最终动作展开，作为实现和排查时的验收基线。所有场景都必须遵守两个前置判断：

1. 当前系统代理 target 必须匹配 Bifrost managed target 或 last runtime target；不匹配时一律 `SkippedExternalOwner`。
2. macOS network services 不可读时一律 `Retrying`；禁止退回 `scutil --proxy` 聚合状态做 destructive cleanup。

### 场景总览

| 场景 | 主要平台 | 触发层 | 关键判断 | 最终动作 |
| --- | --- | --- | --- | --- |
| 正常 stop / clean shutdown | macOS / Windows | Layer 0 | 当前代理仍匹配 Bifrost target | restore original 或 disable stale proxy |
| 系统重启且来不及优雅退出 | macOS | Layer 3 + Layer 4 | 旧 runtime dead，系统代理残留 | LaunchDaemon/startup recovery 清理 |
| 系统刚启动 network services 未 ready | macOS | Layer 3 | service list 不可读或为空 | 保留 state，持续 retry |
| sleep/wake 后 listener alive | macOS | Layer 1 | listener probe 成功 | `KeptManagedProxy` |
| sleep/wake 后主进程已死，runtime 可托管 | macOS | Layer 1 | listener dead，daemon/desktop restartable | 先自动重启主进程，成功则 `RestartedRuntime` |
| sleep/wake 后主进程已死，runtime 不可托管 | macOS | Layer 1 | listener dead，foreground/clean stop/不可信 runtime | restore original 或 disable stale proxy |
| sleep/wake 后 listener alive 但上游网络未 ready | macOS | Layer 1 + Phase 6 | listener alive，出现 ENETUNREACH/EADDRNOTAVAIL | 不恢复，只写 `network_stack_unready_summary` |
| sleep/wake 后 network services 不可读 | macOS | Layer 1 | 无法可靠读取 per-service proxy | `Retrying` |
| 外部代理抢占 | macOS / Windows | 任意层 | 当前 target 不匹配 Bifrost | `SkippedExternalOwner` |
| helper missing | macOS / Windows | 主进程 watchdog | helper pid missing 或 start_time mismatch | 重启 helper；不直接改系统代理 |
| watcher failed | macOS | helper startup | IOKit watcher 初始化失败 | helper 继续 parent-death cleanup，主进程 wake-gap 兜底 |
| Windows helper parent-death | Windows | Layer 1 / Layer 4 | 无 IOKit，旧 runtime dead | 可托管则先重启；否则 restore/disable |

### 系统正常重启

如果系统给 Bifrost 足够时间优雅退出：

1. 主进程收到 SIGTERM/SIGINT/SIGHUP 或正常 stop。
2. 主进程持有 `.system_proxy.lock`。
3. 逐个 network service 或 Windows 当前代理状态确认 target 是否仍属于 Bifrost。
4. target match 时 restore original；无 original backup 但 target 匹配 last runtime 时 disable stale proxy。
5. 写 `cleanup_started`、`cleanup_restored` 或 `cleanup_disabled_stale_proxy`。
6. owner state 写 `clean_shutdown=true`、`last_cleanup_result=success`。
7. helper 停止 watcher 并退出。

预期结果：系统代理不再指向 Bifrost dead listener，下次启动前 recovery 应快速 no-op。

### 系统重启时未能优雅退出

如果系统重启或进程被强制结束，主进程和 helper 都可能没有机会完成 Layer 0/Layer 1 cleanup。

macOS 处理路径：

1. 开机后 LaunchDaemon one-shot 触发 cleanup。
2. cleanup-daemon 读取 `runtime.json`、`bifrost.pid`、`proxy_state.json`、`proxy_backup.json`。
3. 用 pid + start_time + listener probe 判定旧 runtime 是否仍可信。
4. 旧 runtime dead 且当前 system proxy 指向 Bifrost target 时，执行 restore/disable。
5. 如果 LaunchDaemon 不可用或错过，下一次 `bifrost start` 的 startup recovery 继续覆盖同一逻辑。

Windows 处理路径：

1. 没有 LaunchDaemon one-shot。
2. 依赖 startup recovery 和 Windows helper parent-death cleanup。
3. 当前代理指向 Bifrost target 且旧 runtime dead 时，按 restart-before-restore 决策处理。

### 系统刚启动但 network services 未 ready

该场景只适用于 macOS `networksetup` 路径。

1. LaunchDaemon/startup recovery 已确认旧 runtime 可能 dead。
2. `networksetup -listallnetworkservices` 失败或返回空。
3. 不能执行 restore/disable，不能删除 state。
4. 写 `cleanup_retrying` 或 `network_stack_unready_summary`，owner state 写 `last_cleanup_result=retrying`。
5. 后台 retry；service list ready 后重新判断 target 和 listener。

预期结果：

- 后续仍指向 Bifrost dead listener：restore/disable。
- 后续外部代理已接管：skip external owner。
- 长时间不可读：status/doctor 显示 network services unready，而不是误报恢复成功。

### Sleep/Wake 后 listener 正常

1. helper 收到 `kIOMessageSystemWillSleep`，立即 ack，并只写轻量 event。
2. `kIOMessageSystemWillPowerOn` 只记录 early wake，不做 restore。
3. `kIOMessageSystemHasPoweredOn` 后 debounce 2 秒。
4. 调用 `reconcile_system_proxy_after_wake(power_notification, ...)`。
5. 当前 proxy 指向 Bifrost target，listener probe 成功。
6. outcome=`KeptManagedProxy`，不修改系统代理。

如果同时出现上游请求 `ENETUNREACH`、`EADDRNOTAVAIL`、DNS 失败或 timeout，只能由 request error aggregation 写 `network_stack_unready_summary`。listener alive 时禁止 restore。

### Sleep/Wake 后主进程已死且代理残留

这是用户反馈现场的核心场景：电脑没有重启，只是合盖/休眠；唤醒时系统代理仍指向 Bifrost，但主进程和 listener 已经不可用。

处理顺序必须是 restart-before-restore：

1. helper wake watcher 或 parent-death poll 发现旧 runtime dead。
2. coordinator 串行进入同一决策，避免 wake reconcile 和 parent-death cleanup 双写。
3. 确认当前系统代理仍匹配 Bifrost target。
4. 如果 owner/runtime state 表明上次是 `daemon` 或 `desktop` 托管运行时，并且没有 clean stop marker：
   - 写 `runtime_restart_considered`。
   - 启动新的托管主进程。
   - 写 `runtime_restart_started`。
   - 10 秒内等待 listener 和新 pid/start_time 可确认。
   - 成功后写 `runtime_restart_succeeded` 与 `wake_notification_reconcile_restarted_runtime`，outcome=`RestartedRuntime`。
   - 保持系统代理指向 Bifrost。
5. 如果上次是 `foreground`、用户 clean stop、runtime identity 不可信、binary path/data dir 不可信、外部代理已接管，或重启失败/超时：
   - 写 `runtime_restart_skipped` 或 `runtime_restart_failed`。
   - 进入 guarded restore/disable。

该策略的意图是：托管运行时异常死亡时优先自愈服务；只有确认不能安全自愈时，才通过恢复/关闭系统代理先救用户网络。

### Sleep/Wake 后 network services 不可读

1. helper 已收到 wake notification。
2. shared reconcile 读取 per-service proxy 失败。
3. outcome=`Retrying`，reason=`network_services_unready`。
4. 不 probe 或不信任 destructive 决策，不 restore、不 disable。
5. retry 成功后再按 target/listener/restartable runtime 分支处理。

### 外部代理抢占

任意触发路径下，只要当前系统代理不匹配 Bifrost target，就必须：

1. 写 `cleanup_skipped_external_owner` 或 `reconcile_completed` with `decision=skipped_external_owner`。
2. 不 restore、不 disable、不自动重启主进程接管代理。
3. status/WebUI 显示 occupied by another proxy。
4. doctor summary 明确 Bifrost 没有处理外部代理。

这条规则覆盖 Surge、Clash、VPN、用户手动代理，也覆盖 Windows 当前代理状态。

### helper 或 watcher 不可用

helper missing：

1. 主进程 watchdog 以 `pid 存在 && start_time 匹配` 作为 helper alive 权威判断。
2. pid missing 连续 3 次或 start_time mismatch 时允许重启 helper。
3. heartbeat stale 只写 warning，不直接重启。
4. watcher failed 不触发 helper 重启。

watcher failed：

1. helper 写 `wake_notification_watcher_failed`。
2. helper 继续 parent-death cleanup。
3. 主进程 scheduler wake-gap 作为 macOS fallback。
4. Windows 上 watcher unsupported 是正常状态，不展示 warning。

### 下一次 Bifrost 启动前清理残留

如果 LaunchDaemon/helper 都错过，下一次 `bifrost start` 必须在证书检查、端口冲突检查和新系统代理接管前执行 startup recovery：

1. 读取旧 runtime 和 system proxy state。
2. 旧 runtime dead 且当前代理匹配 Bifrost target。
3. 可托管 runtime 可先尝试自动重启；如果当前命令本身就是新 start，则可直接进入新 runtime 接管流程。
4. 不可重启或重启失败时 restore/disable stale proxy。
5. 清理 stale runtime/pid 后再继续新启动。

### 最终 Outcome 分类

| Outcome | 含义 | 是否改系统代理 | 是否启动主进程 |
| --- | --- | --- | --- |
| `KeptManagedProxy` | target match 且 listener alive | 否 | 否 |
| `RestartedRuntime` | target match、listener dead、托管 runtime 重启成功 | 否 | 是 |
| `RestartSkipped` | 不满足自动重启条件，继续 fallback | 取决于后续 restore | 否 |
| `RestoredDeadListener` | listener dead 且有 original backup | 是 | 否 |
| `DisabledStaleProxy` | listener dead，无 backup 但 target 匹配 last runtime | 是 | 否 |
| `SkippedExternalOwner` | 当前代理不属于 Bifrost | 否 | 否 |
| `Retrying` | network services 或可重试 IO 暂不可用 | 否 | 否 |
| `Failed` | 不可重试错误或 fallback 失败 | 否或部分失败 | 否 |
