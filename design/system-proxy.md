# System Proxy Ownership

> 后续可靠性、结构化 lifecycle event、helper heartbeat/watchdog、`bifrost doctor system-proxy` 诊断包等增量方案见 [System Proxy Reliability and Diagnostics](./system-proxy-reliability-diagnostics.md)。

## 背景

Bifrost 的 System Proxy 只应管理自己写入的系统代理配置。用户同时运行 Surge、Clash、系统级 VPN/代理时，Bifrost 必须：

- 能真实反映当前 OS 代理状态；
- 不把外部代理误判成自己的、错误清理外部代理；
- `system-proxy disable`、Admin UI 关闭、`bifrost stop`、崩溃/重启/睡眠恢复等所有清理入口都只清理归属于本 runtime 的系统代理。

历史上 Bifrost 只有一次性 enable/disable，缺归属判定和跨进程锁，导致：外部代理被误关、Bifrost 崩溃后 OS 代理长时间指向死端口、restart 期间出现断网窗口。本方案沉淀多轮线上回归的最终形态。

## 用户目标验证清单

### 必须实现

- `SystemProxyManager::enable` 使用两阶段 `applied` 标记写入 `proxy_state.json`；`proxy_backup.json` 兼容旧恢复。
- 关闭/恢复路径先判断归属，`OwnedByOther` 时保持外部代理不变。
- `bifrost start` 在证书/端口冲突检查之前同步执行 `SystemProxyManager::recover_from_crash`。
- 每个 Bifrost runtime 都启动跨平台 lifecycle helper（独立进程组/DETACHED_PROCESS），统一负责系统代理与独立 CLI proxy 环境的退出清理；macOS 启用系统代理时再后台异步安装 cleanup LaunchDaemon（用户取消授权不阻塞主服务）。
- Windows lifecycle helper 使用 `CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS`；父进程崩溃时 helper `Drop::detach()` 继续兜底。
- helper 身份判定使用 PID + start_time 双因子（容差 2000ms），避免 PID 复用误清理。
- 所有 macOS 系统代理写入通过 `data_dir/.system_proxy.lock`（`O_NOFOLLOW` + fd `fchmod(0666)`）跨进程串行化。
- `bifrost restart` / `bifrost upgrade` 在旧 runtime 仍持有系统代理时，为新 daemon argv 附加 `--system-proxy` + bypass，并写 `preserve_for_restart` marker，避免断网窗口。
- Admin API `GET /api/proxy/system` 返回 live (`enabled/host/port/managed_by_bifrost`) 与 desired (`configured_enabled/configured_bypass`)。
- CLI `system-proxy enable/disable` 优先调用 Admin API，Admin 不可达时回退本地 `SystemProxyManager`，且能用 `runtime.json` fallback expected target。

### 必须不破坏

- 用户从未启用系统代理时，Bifrost 不应写入 OS 代理或安装 LaunchDaemon；通用 lifecycle helper 仍启动，以保护运行中可能随后安装的独立 CLI proxy 环境。
- Linux 不支持系统代理（`SystemProxyManager::is_supported()` 返回 false），不写 OS 代理、不装 LaunchDaemon；仍写跨平台 restart/stop shutdown marker，并启动通用 helper 负责 CLI proxy 环境清理。
- 外部代理（Surge/Clash/系统 VPN）在 Bifrost 任何清理路径下都不能被关闭；`disable` 请求成功且 `managed_by_bifrost=false` 时不报错 `System proxy is still enabled`。
- `bifrost restart` 期间 OS 代理不能出现 disable→enable 的可感断网。
- 已启用 helper 时 `bifrost stop` 必须先前台清理系统代理，只有 cleanup 成功才发送 SIGTERM。

### 必须真实验证

- macOS 真实 GUI 授权安装/卸载 LaunchDaemon；同一 program/data dir/版本再次启动不重复弹授权。
- Windows x86 real self-update replacement 在原生 runner 上跑通 `bifrost upgrade`（`E2E Shell (x86_64-pc-windows-msvc)` job）。
- 睡眠恢复、崩溃恢复、无 `proxy_state.json` 但 `runtime.json` 匹配现场三条恢复路径都真实回归。
- CLI `system-proxy status` 输出 live/managed/configured 三段。

## 产品语义

### 归属判定是所有清理动作的前提

- `ProxyBackup::target_matches(host, port)` 覆盖 loopback alias（`127.0.0.1` / `localhost` / `0.0.0.0` / IPv6 wildcard）与端口匹配。
- `decide_managed_state_recovery` 覆盖 `applied=false` + 未出现 target → 丢弃 pending；`applied=false` + 已出现 target → 恢复 original；`applied=true` → 归属判定后恢复。
- `bifrost stop` 只在 `runtime.json` host/port 与当前 OS 代理匹配时才清理。
- `recover_from_crash` 在 state/backup 缺失但 `runtime.json` 匹配时执行 failsafe 关闭；不匹配则保留外部代理。

### 两阶段 apply 标记防止误恢复

`proxy_state.json` 写入次序：
1. `applied=false` 落盘 → 写 macOS network service → `applied=true` 落盘。
2. Recovery 只在 `applied=true` 或"apply 中途已出现 target"时恢复；否则丢弃 pending。

### 运行期 reconcile 与 wake-gap

- 后台 reconcile 线程周期性复核 (30s)。
- macOS wake-gap reconcile：检测调度器 tick 时间跳变 > 10s 时立即触发。
- 只在 desired enabled 时 apply；desired flag 由 Admin API/Web UI 更新。

### lifecycle helper 与 LaunchDaemon 分工

| 角色 | 触发 | 生命周期 |
| --- | --- | --- |
| lifecycle helper | 主进程 spawn，`SIGKILL`/崩溃时兜底 restore | 主进程独立进程组子进程；`Drop::detach()` |
| cleanup LaunchDaemon (macOS) | launchctl bootstrap/kickstart/系统启动 | one-shot，`RunAtLoad=true`，无 `KeepAlive` |

- helper 使用 `retry_with_policy(window=60s, interval=5s)`；`is_retryable_recovery_error` 区分 transient vs 不可重试。
- LaunchDaemon 处理"系统 network service 未 ready"时持续定时重试直到可读。

### restart / upgrade handoff

- `bifrost restart` 写 `preserve_for_restart` marker，旧 daemon/旧 helper 都跳过清理。
- fresh daemon `start --system-proxy` 在 marker + 旧 runtime 同时存在时跳过启动前 crash recovery；reconcile 线程也跳过初始 recovery。
- shutdown marker 只属于旧 runtime 退出窗口；新 runtime 写入 `runtime.json` 或 readiness pipe 确认后消费。

## 技术细节

### 关键源码

| 文件 | 责任 |
| --- | --- |
| `crates/bifrost-core/src/system_proxy.rs` | `SystemProxyManager` enable/disable/recover_from_crash、state/backup 序列化、`decide_managed_state_recovery` |
| `crates/bifrost-core/src/system_proxy_launchd.rs` | plist 生成/解析、`recover_if_no_live_runtime_with_startup_retry` |
| `crates/bifrost-core/src/system_proxy_recovery.rs` | 共享 `retry_with_policy(window, interval, closure)`、`is_retryable_recovery_error` |
| `crates/bifrost-core/src/process_start_time.rs` | 跨平台 `current_process_start_time_ms` / `get_process_start_time_ms(pid)` / `start_times_match` |
| `crates/bifrost-admin/src/state.rs` | `SystemProxyLifecycleHelperState::ensure_started/stop/detach` |
| `crates/bifrost-admin/src/handlers/proxy.rs` | `GET/PUT /api/proxy/system`、`/api/proxy/system/launchd` |
| `crates/bifrost-cli/src/commands/system_proxy.rs` | CLI enable/disable/status/cleanup-daemon subcommands，Admin API fallback |
| `crates/bifrost-cli/src/commands/restart.rs` | restart arg 构造，`runtime_system_proxy_host` wildcard→loopback |
| `crates/bifrost-cli/src/commands/stop.rs` | 前置 cleanup、写 `foreground_cleanup` marker |
| `crates/bifrost-cli/src/commands/upgrade.rs` | Windows self-replace via PowerShell helper |
| `crates/bifrost-cli/src/process.rs` | `runtime_info_system_proxy_target` / `runtime.started_at_ms` |
| `crates/bifrost-core/src/shell_proxy.rs` | 与 shell rc 分离，不复用 system proxy 路径 |

### 关键数据结构

```rust
pub struct ProxyBackup { pub enable: bool, pub host: String, pub port: u16, pub bypass: Vec<String> }

pub struct ManagedProxyState {
    pub target: ProxyBackup,
    pub original: ProxyBackup,
    #[serde(default = "managed_proxy_state_applied_default")] // true
    pub applied: bool,
}

pub enum ProxyOwnership { OwnedByBifrost, OwnedByOther, Disabled }
```

### 跨平台差异

- **macOS**：`networksetup -getwebproxy/-getsecurewebproxy` 逐 network service 归属判定；`.system_proxy.lock` 保护写入；GUI 授权走 `osascript ... with administrator privileges`；network service 读取并发、写入按 service 有界并行（默认 4）。
- **Windows**：`sysproxy` / 注册表；helper 用 `CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS`；不需要 file lock。
- **Linux**：`is_supported() = false`；仅 CI 跑 `bifrost restart` 无系统代理链路，保证除系统代理外行为对齐。

## CLI + Web + Admin API

### CLI

```bash
bifrost system-proxy enable [--host <ip>] [--port <p>] [--bypass <cidr>]
bifrost system-proxy disable
bifrost system-proxy status                 # 输出 live + Managed by Bifrost + Configured
bifrost system-proxy cleanup-daemon         # 隐藏子命令，LaunchDaemon 调用
bifrost system-proxy repair-lock --data-dir <dir>  # 迁移 root-owned strict lock
bifrost system-proxy launchd status|install|uninstall
```

行为：CLI 优先调 Admin API；Admin 不可达时回退本地 manager；`disable` 遇 `OwnedByOther` 时读取 `runtime.json` 的 expected target 重试 explicit disable。

### Web UI

- Settings → Proxy：开关按 `configured_enabled` 显示，subtitle 显示 live `enabled + managed_by_bifrost`；不一致时 pending/warning。
- `Boot/Shutdown Cleanup` 开关调用 `/api/proxy/system/launchd` install/uninstall，`needs_upgrade_reason` 决定是否弹迁移提示。
- StatusBar / Traffic toolbar 同样使用 `configured_enabled`。

### Admin API

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/api/proxy/system` | `enabled`, `host`, `port`, `bypass`, `managed_by_bifrost`, `configured_enabled`, `configured_bypass` |
| PUT | `/api/proxy/system` | `{enabled, host?, port?, bypass?}`；更新 desired flag，触发 lifecycle helper + LaunchDaemon 检查 |
| GET | `/api/proxy/system/launchd` | `installed_version`, `installed_mode`, `current_version`, `needs_upgrade`, `needs_upgrade_reason`, plist 路径 + load 状态 |
| POST | `/api/proxy/system/launchd/install` | GUI 授权安装 |
| POST | `/api/proxy/system/launchd/uninstall` | GUI 授权卸载 |

## Sync 边界

系统代理是每台设备的本地状态，不参与 Sync/导入导出/分享；`config.toml` 中的 `system_proxy.enabled` 仅保存本机 desired 偏好。crash recovery / cleanup-daemon 只允许修正 OS 现场，不允许把 `config.toml` 中 desired 改为关闭。

## Phase 1-4

### Phase 1：归属判定与状态文件

- 新增 `ManagedProxyState` + 两阶段 `applied` 标记。
- 关闭/enable 路径归属判定；返回 `OwnedByOther`。
- `bifrost stop` 严格依赖 runtime host/port + live OS 匹配。

### Phase 2：跨进程串行化 + startup recovery

- `.system_proxy.lock` (`O_NOFOLLOW` + fd `fchmod`)；覆盖所有写入入口。
- `bifrost start` 前置 `recover_from_crash`；`runtime.json.started_at_ms` PID + start_time 双因子。

### Phase 3：lifecycle helper + cleanup LaunchDaemon

- macOS + Windows helper（DETACHED/独立进程组）。
- 共享 `retry_with_policy`；network service list 空视为 readiness 持续重试。
- LaunchDaemon 后台异步授权安装；`Boot/Shutdown Cleanup` 显式管理入口。

### Phase 4：restart handoff + Admin 一致性

- `preserve_for_restart` marker；fresh daemon 跳过 recovery；旧 helper 跳过 cleanup。
- Admin API `configured_enabled` / `configured_bypass` 与 CLI 对齐；WebUI Settings/StatusBar/Traffic 全部使用 configured 驱动开关状态。
- CLI `system-proxy` 优先 Admin API；Admin 不可达时本地 fallback 支持 runtime target。

## 测试方案

### 单元测试（`cargo test -p bifrost-core system_proxy`）

- `ProxyBackup::target_matches` — loopback alias / 端口不匹配。
- `decide_managed_state_recovery` — 覆盖 `applied=false` 未出现 target / 中途出现 target / `applied=true` 归属判定。
- `load_last_runtime_proxy_target` / `current_proxy_matches_target` — `runtime.json` fallback、`0.0.0.0` → loopback、非法端口忽略。
- `recover_if_no_live_runtime_with_startup_retry` — cleanup-daemon 启动期 retry 分类；空 service list 视为 readiness 持续重试。
- `system_proxy_lock_is_world_writable_after_creation` / `system_proxy_lock_rejects_symlink` — 真实创建 lock 断言 `0o666`；预置 symlink 断言 `O_NOFOLLOW` 拒绝。
- `runtime_identity_is_not_alive_when_start_time_mismatches` (Unix) — start_time mismatch 时不能因 PID 存活跳过 cleanup。
- `process_is_running_returns_false_for_missing_pid_without_shelling_out` — 不用 `/bin/kill -0`。
- `remove_stale_runtime_files_removes_runtime_and_pid_files`。
- `last_runtime_target_has_live_listener_resolves_localhost`。
- `process_start_time::tests::current_process_start_time_is_some` / `start_times_match_within_tolerance` / `start_times_match_outside_tolerance` — macOS / Linux / Windows。
- `system_proxy_recovery::tests::retry_with_policy_returns_after_success` / `retry_with_policy_gives_up_after_window`。
- `system_proxy_launchd::tests::*` — plist 版本元数据、ProgramArguments 解析、label 校验、startup retry 分类。

### CLI/Admin 单元测试

- `cargo test -p bifrost-cli commands::stop::tests` — wildcard listen host → loopback 映射。
- `cargo test -p bifrost-cli commands::system_proxy::tests` — runtime target fallback、`OwnedByOther` 重试 explicit disable。
- `cargo test -p bifrost-cli commands::restart::tests` — `runtime_system_proxy_host` wildcard/IPv6 映射；upgrade 默认自重启附加 `--system-proxy` / `--proxy-bypass`。
- `cargo test -p bifrost-admin proxy::tests` — disable 验证外部代理仍启用视为成功。
- `cargo test -p bifrost-admin lifecycle_helper_program` — helper 程序路径 fallback。

### E2E 测试

- `e2e-tests/tests/test_system_proxy_e2e.sh` — 外部代理归属回归、无 backup/state 但 runtime target 匹配启动前恢复、Admin API 运行中启用 helper 检查、Admin API 结构断言、cleanup-daemon 无状态快速退出、fake `networksetup` readiness 回归、helper 崩溃兜底、helper 禁用回归。
- `e2e-tests/tests/test_stop_restart_shutdown_marker.sh` — Linux/macOS 无系统代理 restart 跨平台回归。
- `e2e-tests/tests/test_upgrade_restart_e2e.sh` + `test_upgrade_local_restart_e2e.sh` — Windows x86 由 CI job `E2E Shell (x86_64-pc-windows-msvc)` 执行。

### human_tests

`human_tests/cli-system-proxy.md`：

- TC-SP-01：Surge/外部代理归属回归 — CLI disable 与 stop 不清理外部代理。
- TC-SP-02：睡眠恢复可用回归。
- TC-SP-03：lifecycle helper 崩溃兜底回归 — 强杀主进程后 helper 恢复。
- TC-SP-04：无 backup/state 但 runtime target 匹配回归。
- TC-SP-05：GUI 授权安装 LaunchDaemon（启用系统代理后异步弹授权）。
- TC-SP-06：Admin API / Web UI 打开系统代理后自动检查 LaunchDaemon 与 helper。
- TC-SP-07：崩溃/重启 cleanup 后 Web UI 开关仍保留 configured enabled。
- TC-SP-08：normal stop 前置 cleanup 提示，无 background cleanup / SIGKILL。
- TC-SP-09：`bifrost restart` 保持系统代理指向 fresh daemon 全程不断网。
- TC-SP-10：LaunchDaemon 已安装且 program/data dir/mode 一致时再次启动不重复弹授权，即使 `installed_version` 与 `current_version` 差异也不应仅因版本弹授权。

约束：临时 `BIFROST_DATA_DIR`；系统代理用例明确涉及系统代理，可省略 `--no-system-proxy`。

### 日志验证

必须出现（按场景）：`checking for stale system proxy state before startup`、`System proxy crash recovery check starting`、`system proxy scheduler or wake gap detected; reconciling immediately`、`system proxy lifecycle cleanup helper started` / `system proxy lifecycle helper started after Admin API enable`（含 `helper_program` + helper pid）、`waiting_for_system_proxy_lock` / `acquired_system_proxy_lock`、`system proxy LaunchDaemon cleanup install starting asynchronously` / `already installed and current`、`system proxy launchd cleanup daemon started`、`system proxy shutdown restore starting; stopping reconcile first`、`Restoring macOS system proxy to saved original state`、`Selected macOS network services still pointing at Bifrost target for restore`、service 级 `elapsed_ms`。失败场景对应 `failed to restore system proxy` / `system proxy reconcile failed`。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `rust-project-validate`
- 本机 no-local-coverage 时不跑 `make coverage`；依赖 CI Linux/macOS/Windows 分片。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核 diff：storage/admin/CLI/Web/core/launchd/helper/restart/upgrade。
- 重点 review：是否所有清理入口都做归属判定；是否所有写入入口都持有 `.system_proxy.lock`；helper Windows/Unix spawn 是否 detached；restart handoff 是否可能出现旧 helper 抢跑清掉 fresh runtime。
- 复测：核心单测、`test_system_proxy_e2e.sh`、`test_stop_restart_shutdown_marker.sh`。

### 第 2 轮

- 复核第 1 轮问题修复。
- 再次检查 `git diff`、human_tests 索引与设计文档一致。
- 重点 review：Admin API `disable` 未真正 converge 时的 warning 与 helper 保留策略；`installed_version` 差异不应触发 `needs_upgrade`。
- 复测：真实 macOS GUI 授权流；Windows CI x86 self-update；`bifrost restart` 保持系统代理回归。

## 风险与决策

- **helper 路径漂移**：`current_exe()` 在开发环境 rebuild 后可能失效；实现使用 `argv[0]` fallback，并支持 `BIFROST_SYSTEM_PROXY_LIFECYCLE_HELPER_PROGRAM` 显式指定。
- **多网卡 macOS**：service 级 enable/restore 有界并行（默认 4），GUI 授权仍串行避免并发弹窗。
- **network service list 空**：视为 readiness 持续重试，禁止退回 `scutil --proxy` 聚合状态后清理证据。
- **PID 复用**：任何跳过 cleanup 的判断必须同时校验 PID alive + start_time 匹配 + listener alive；缺一都执行 guarded crash recovery。
- **restart 断网窗口**：`--system-proxy` 追加 + `preserve_for_restart` marker + fresh daemon 跳过启动前 recovery，三者缺一都可能造成 disable→enable 断网。
- **Linux**：Linux 不写系统代理；`SystemProxyManager::is_supported()` = false；仅在 `bifrost-core` 编译期保留 process_start_time 实现供 CI 单测。
- **测试 flake**：Windows detached daemon readiness 通过 `BIFROST_DAEMON_READY_TIMEOUT_SECS` 拉长；失败时上传 `.bifrost-upgrade-*.log`。
- **旧 root-owned lock**：迁移路径 `system-proxy repair-lock --data-dir` 使用 `O_NOFOLLOW` + fd chmod 修复权限，走 GUI 授权。

## 依赖项

- macOS：`networksetup`、`scutil --proxy`、`/bin/launchctl`、`/usr/bin/osascript`、`/Library/LaunchDaemons` plist。安装/卸载需管理员授权；启动后异步授权，取消不影响主服务。
- Windows：`sysproxy` + 注册表。
- Linux：无系统代理写入；CI 保留 `bifrost restart` 无系统代理链路。
- WebUI：复用 `SystemProxyStatus` + 新 `SystemProxyLaunchdStatus`。

## 文档更新

- `human_tests/cli-system-proxy.md` — 追加外部代理归属、helper 崩溃兜底、LaunchDaemon 授权、`restart` 保持代理、`installed_version` 不重复授权用例。
- `human_tests/readme.md` — 同步 CLI 系统代理用例数与说明。
- `design/system-proxy-reliability-diagnostics.md` — 后续 lifecycle event / doctor 增量方案。
- `design/system-proxy-launchd-oneshot.md` — LaunchDaemon one-shot 决策与 startup retry。
