# System Proxy LaunchDaemon One-Shot Optimization

## 状态

已落地（2026-06-17）。one-shot plist、`SystemProxyLaunchdMode`、`needs_upgrade_reason`、cleanup-daemon 单次退出与 lifecycle helper 兜底均已在 `bifrost-core` / `bifrost-cli` 中实现，并被 E2E 与 human_tests 覆盖。本文档保留为设计纪要与运维口径基线；后续行为变更需要新的设计单。

## 背景

macOS 上 `bifrost` 启用系统代理后会安装一个 system LaunchDaemon（`/Library/LaunchDaemons/com.bifrost.system-proxy-cleanup.plist`），用来在系统重启或异常场景下清理残留的系统代理。旧版 plist 同时写 `RunAtLoad=true` 与 `KeepAlive=true`，导致 cleanup daemon 常驻运行：即便只做启动兜底也需承担完整 bifrost binary 的 Tokio runtime、日志、依赖链接的空闲 RSS（实测 5-15 MiB）。

本优化把 LaunchDaemon 改为 one-shot：系统加载或手动 kickstart 时执行一次恢复检查后立即退出。运行中主进程崩溃继续由已有 lifecycle helper 兜底；正常 stop 依旧由主进程 graceful restore。

## 用户目标验证清单

### 必须实现

- plist 默认 one-shot：`RunAtLoad=true`，不写 `KeepAlive`。
- `system-proxy cleanup-daemon` 执行一次恢复检查后立即退出。
- 旧含 `KeepAlive=true` 的 plist 被识别为 needs-upgrade。
- Bifrost 版本变化但 program / data_dir / mode 均匹配时不触发 needs-upgrade。
- status / Web UI 不把“无 cleanup-daemon PID”视为异常。
- lifecycle helper 保持启用，覆盖运行期主进程异常退出。

### 必须不破坏

- 用户取消 LaunchDaemon 授权不阻塞主服务启动。
- 启动时系统代理开启后仍自动检查/安装 LaunchDaemon。
- 运行中 Admin API/Web UI 打开 system proxy 后自动检查/安装 LaunchDaemon 并启动 lifecycle helper。
- 外部代理 ownership 保护不变。
- 正常 stop / SIGTERM / listener exit restore 顺序不变。
- 下次启动前 crash recovery 不变。

### 必须真实验证

- macOS launchd 真实 install / status / kickstart / process absence。
- 主进程 `kill -9` 后 lifecycle helper 恢复 system proxy。
- 旧 KeepAlive plist 升级为 one-shot plist。
- 版本号不一致但 program/data_dir/one-shot mode 一致时不重新安装。
- Web UI / Admin API 打开 system proxy 后自动安装 one-shot LaunchDaemon。

## 产品语义

### 职责分工

| 场景 | 负责路径 | 说明 |
| --- | --- | --- |
| 系统启动后发现上次残留 Bifrost system proxy | one-shot LaunchDaemon | `RunAtLoad` 执行一次恢复检查后退出 |
| 安装/升级 LaunchDaemon 后即时检查 | one-shot LaunchDaemon | `bootstrap` / `kickstart` 触发；主服务活着时跳过 |
| Bifrost 主进程 `kill -9` / panic / OOM | lifecycle helper | 父 PID 连续 3 次不可见后恢复 |
| 正常 stop / SIGTERM | 主进程 graceful restore | 先停 reconcile，再 restore |
| 外部代理接管 system proxy | ownership 判断 | 所有恢复/关闭路径都不得误清理 |
| 用户取消 LaunchDaemon 安装授权 | 主服务继续运行 | system proxy 可用，但系统重启兜底缺失，UI/日志提示 |

### 决策摘要

1. LaunchDaemon 从常驻改为 one-shot（`RunAtLoad=true`，不写 `KeepAlive`）。
2. 只负责系统启动 / bootstrap / kickstart 后的遗留 system proxy 恢复检查。
3. 运行期崩溃仍归属 lifecycle helper。
4. 正常 stop / SIGTERM / listener exit 仍归属主进程 restore，保持外部代理 ownership 保护。
5. `needs_upgrade` 不再由版本号单独决定：只要 plist 指向的 Bifrost 程序路径、data dir、运行模式仍正确，版本号变化不触发。
6. 旧 `KeepAlive=true` plist 必须被识别为 needs-upgrade（即使 Bifrost 版本号相同）。

### macOS launchd 支持性

- `RunAtLoad=true`：job 被加载时启动一次。
- 不写 `KeepAlive`：job 退出后 launchd 不重新拉起。
- 不使用 `LaunchOnlyOnce=true`（禁止同一 boot 内再次 kickstart 验证）。
- 不使用已废弃 `OnDemand`。
- 依赖：`/bin/launchctl`、`/usr/bin/osascript`、`networksetup`；system domain LaunchDaemon 不依赖用户 login session。

### OS 覆盖

- macOS：完整支持。
- 非 macOS：no-op / unsupported。

## 技术细节

### plist 渲染（`crates/bifrost-core/src/system_proxy_launchd.rs`）

- `render_launchd_plist` 生成的 plist 包含 `<key>RunAtLoad</key><true/>`（line 140 附近），不包含 `<key>KeepAlive</key>`（单测 line 942/943 断言）。
- 环境变量：仍写入 `BIFROST_LAUNCHD_INSTALLED_VERSION`；`BIFROST_LAUNCHD_MODE=oneshot` 为 planned, not yet shipped as of 2026-06-17。

### cleanup-daemon 入口（`crates/bifrost-cli/src/commands/system_proxy.rs`）

- `run_system_proxy_cleanup_daemon` 直接消费 `bifrost-core::system_proxy_launchd::recover_if_no_live_runtime_with_startup_retry(data_dir)`。
- 返回类型 `SystemProxyLaunchdRecoveryOutcome`（`Recovered` / `Skipped`）；`Skipped` 合并了 live runtime 与 live proxy target 两种命中。失败通过 `Err` 上抛并在 CLI 侧 `warn!` 记录。
- 细分为 `SkippedLiveRuntime` / `SkippedLiveProxyTarget` / `Failed` 的变体为 planned, not yet shipped as of 2026-06-17。
- 退出码：`0`（成功或安全跳过）；失败也返回 `0`，理由是 launchd 不应把权限/系统状态失败解释为需要自动重试。
- 不再创建长期 Tokio runtime，不再注册 SIGTERM/SIGINT/SIGHUP 后进入 3600 秒 heartbeat loop。

### `SystemProxyLaunchdStatus`（`crates/bifrost-core/src/system_proxy_launchd.rs` line 42 附近）

```rust
pub struct SystemProxyLaunchdStatus {
    pub installed_mode: Option<SystemProxyLaunchdMode>, // line 42
    pub needs_upgrade: bool,
    pub needs_upgrade_reason: Option<String>,           // line 45
    // ...
}

pub enum SystemProxyLaunchdMode {   // line 51
    OneShot,
    KeepAlive,
    Unknown,
}
```

- `parse_installed_plist` 识别 `keep_alive` / `run_at_load`（line 802/803）。
- `installed_launchd_mode(&ParsedLaunchdPlist)` 落地在 line 814。
- `launchd_needs_upgrade_reason(...)` 返回可读升级原因（line 824）。

### 升级判定规则

| 条件 | needs_upgrade |
| --- | --- |
| plist 不存在 | false（not installed） |
| program 不匹配 | true |
| data_dir 不匹配 | true |
| installed_version 不匹配，但 program / data_dir / mode 均匹配 | false（仅展示版本差异） |
| installed_version 不匹配，且 program / data_dir / mode 任一不匹配 | true（由结构性不匹配触发） |
| KeepAlive=true | true（reason: `installed plist uses legacy KeepAlive mode`，line 841） |
| 缺少 RunAtLoad=true | true（reason: `installed plist is missing RunAtLoad one-shot mode`，line 844） |
| plist 解析失败 | true（`Unknown`） |

单测覆盖：`keepalive_plist_requires_upgrade_to_oneshot`、`missing_run_at_load_requires_upgrade`、`oneshot_plist_reports_current_mode`。

### 生命周期 helper（`crates/bifrost-cli/src/commands/system_proxy.rs::run_system_proxy_lifecycle_helper`）

- 监听 SIGTERM/SIGINT/SIGHUP，2 秒轮询父进程；连续 3 次不可见判定异常退出。
- 触发 `SystemProxyManager::recover_from_crash(data_dir)`（守护当前 data dir 的 managed target）。
- 不依赖 LaunchDaemon 常驻。

### 状态展示

- CLI `system-proxy launchd status`：`Loaded` 只表示 launchd job 已被 bootstrap，不表示 cleanup-daemon 常驻。
- Web UI `Boot/Shutdown Cleanup` 文案：使用 `installed / loaded / current / one-shot`，避免 `running / daemon alive`。
- 若 `launchctl print system/<label>` 失败但 plist 存在，显示 `installed=true loaded=false`。

## CLI / Web / Admin API

### CLI

- `bifrost system-proxy launchd install --data-dir <dir>`：写 plist + bootstrap + enable + kickstart。
- `bifrost system-proxy launchd status`：显示 installed / loaded / current / mode / needs_upgrade_reason。
- `bifrost system-proxy cleanup-daemon --data-dir <dir> --installed-version <version>`：one-shot 恢复检查后退出（内部 helper，也是 plist ProgramArguments）。
- `bifrost system-proxy lifecycle-helper --parent-pid <pid>`：运行期崩溃兜底。

### Web UI

- Settings → System Proxy → Boot/Shutdown Cleanup 卡片显示：安装状态、加载状态、mode（隐式）、needs_upgrade_reason 文案。
- 授权取消时提示 “cleanup protection 未安装”。

### Admin API

- `POST /_bifrost/api/system-proxy/launchd/install` / `.../uninstall`。
- `GET /_bifrost/api/system-proxy/launchd/status`：返回 `installed / loaded / installed_version / current_version / installed_mode / needs_upgrade / needs_upgrade_reason / message`。
- `installed_mode` 已随 status 一并返回；Web UI 未单独渲染 mode 徽标（planned, not yet shipped as of 2026-06-17）。

## Sync 边界

- LaunchDaemon 安装状态、mode、needs-upgrade reason 都是本机 macOS-only 概念，不通过 Sync 分发。
- 用户在 A 设备安装了 one-shot LaunchDaemon，不影响 B 设备的 plist 状态。

## Phase 1 - 4

### Phase 1：plist 与 cleanup-daemon 改造

- 移除 `KeepAlive`，保留 `RunAtLoad`。
- `run_system_proxy_cleanup_daemon` 改为 one-shot。
- 保留 lifecycle helper。

### Phase 2：needs-upgrade 判定重写

- 引入 `SystemProxyLaunchdMode` / `installed_mode` / `needs_upgrade_reason`。
- 删除 version-only needs_upgrade 条件。
- 覆盖旧 KeepAlive、缺少 RunAtLoad、program/data_dir 不匹配、plist 解析失败等情况。

### Phase 3：状态展示与安装竞态

- CLI / Web UI 文案改造。
- 安装流程写 stop suppression marker，再 bootout / bootstrap / kickstart，避免旧 KeepAlive daemon 在 SIGTERM 时误 restore。
- 自动安装任务保持在 runtime.json 写入 + 服务 ready 之后触发。

### Phase 4：测试与文档

- 单元测试覆盖 plist 渲染、mode 识别、升级判定。
- E2E dry-run 断言。
- human_tests TC-CSP-16/17/18 更新。
- 文档：本 doc、`design/system-proxy.md`、`human_tests/cli-system-proxy.md`、`human_tests/readme.md`。

## 测试方案

### 单元测试

- `crates/bifrost-core/src/system_proxy_launchd.rs::tests`：
  - `launchd_plist_contains_cleanup_daemon_version_and_paths`（含 `RunAtLoad` / 无 `KeepAlive` 断言，line 942/943）。
  - `keepalive_plist_requires_upgrade_to_oneshot`。
  - `missing_run_at_load_requires_upgrade`。
  - `oneshot_plist_reports_current_mode`（`installed_mode == OneShot`，line 1006/1008）。
  - `version_mismatch_with_same_program_data_dir_and_mode_does_not_require_upgrade`。
- `crates/bifrost-cli/src/commands/system_proxy.rs`：
  - `cleanup_daemon_exits_after_startup_recovery_skipped` 或等价可测试 helper。
- `crates/bifrost-admin/src/handlers/proxy.rs::tests`：launchd status handler snapshot。

### E2E

```bash
BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_system_proxy_e2e.sh
```

重点断言：

- plist dry-run 不含 `KeepAlive`、含 `RunAtLoad`。
- `system-proxy cleanup-daemon` one-shot 后命令退出。
- lifecycle helper 崩溃兜底通过。
- `BIFROST_SYSTEM_PROXY_DISABLE_LIFECYCLE_HELPER=1` 场景下下次启动恢复通过。
- 旧 KeepAlive plist 被 status 标记为 needs-upgrade。
- program/data_dir/mode 匹配时不重复弹授权。

### human_tests

`human_tests/cli-system-proxy.md`：

- `TC-CSP-16`：启动后异步安装 LaunchDaemon；授权后 `launchctl print system/com.bifrost.system-proxy-cleanup` 成功；`pgrep -fl "system-proxy cleanup-daemon"` 数秒后无常驻进程。
- `TC-CSP-17`：Web UI 安装/卸载 one-shot LaunchDaemon；一致场景下不重复弹授权；旧 KeepAlive plist 被识别为 needs-upgrade。
- `TC-CSP-18`：运行中通过 Admin API/Web UI 打开 system proxy 后自动安装 one-shot LaunchDaemon，日志包含 `system proxy lifecycle helper started after Admin API enable`。
- 崩溃兜底：`kill -9` 主进程后 helper 在 2 秒 poll / 3 次 miss 后恢复 system proxy。

执行记录需包含临时数据目录、Bifrost 端口、`launchctl print` 摘要、`pgrep -fl "system-proxy cleanup-daemon"` 结果、system proxy enable/disable 前后 host/port、清理结果。

### launchd 真实验证

```bash
./target/debug/bifrost system-proxy launchd install --data-dir "$TEST_DATA_DIR"
launchctl print system/com.bifrost.system-proxy-cleanup
pgrep -fl "system-proxy cleanup-daemon" || true
launchctl kickstart -k system/com.bifrost.system-proxy-cleanup
sleep 2
pgrep -fl "system-proxy cleanup-daemon" || true
```

预期：`launchctl print` 找到 job；kickstart 后日志显示 one-shot recovery completed/skipped/failed；2 秒后无常驻 cleanup-daemon 进程；主服务在运行时日志显示 startup recovery skipped。

### 资源占用验证

```bash
ps -axo pid,rss,vsz,command | rg "system-proxy cleanup-daemon|system-proxy lifecycle-helper"
```

预期：cleanup-daemon 空闲不常驻；lifecycle helper 只在 system proxy enabled 且主服务运行时存在；cleanup-daemon RSS 从可见 5-15 MiB 常驻降为 0 常驻。

### workspace 校验

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

最后运行 `rust-project-validate`。

## Review / Fix / Test 闭环方案

### 第 1 轮

- 复核用户目标：one-shot 覆盖启动兜底 + lifecycle helper 覆盖运行期崩溃 + 主进程覆盖正常停止。
- Review：`RunAtLoad` 存在、`KeepAlive` 缺失、needs_upgrade 不再由版本号触发、Web UI/CLI 文案不再暗示常驻。
- 复测：`launchd` 单测、`system-proxy` E2E、TC-CSP-16/17/18 手测、崩溃兜底手测。

### 第 2 轮

- 复核第 1 轮修复；`git status --short` / `git diff` 收敛。
- Review：安装竞态（stop suppression marker、bootout/bootstrap 顺序）；旧 KeepAlive plist 升级路径；文档一致。
- 复测：`test_system_proxy_e2e.sh` 全跑；resource 占用 before/after。

## 风险与决策

### 风险 1：去掉 KeepAlive 后无法覆盖运行期崩溃

lifecycle helper 承担运行期崩溃兜底；必须复跑 helper 崩溃兜底 E2E 与手测。

### 风险 1.1：Admin API/Web UI 才启用 system proxy 时缺 lifecycle helper

Admin API/Web UI 的 enable 成功路径必须调用共享 lifecycle helper state 的 `ensure_started()`；未在启动时启用 system proxy 的进程也要在 `AdminState` 中持有该 state。

### 风险 2：launchd status 被误判为 unloaded

状态判断只依赖 `launchctl print system/<label>` / plist 存在 / mode 匹配，不依赖 cleanup-daemon PID 常驻。

### 风险 3：旧 KeepAlive plist 不会自动升级

解析 plist 时识别 `KeepAlive=true`，触发重新安装。

### 风险 4：系统关机时没有常驻 LaunchDaemon 收 SIGTERM

正常关机 / stop 由主进程 restore；运行期崩溃由 lifecycle helper；系统重启后遗留由 RunAtLoad one-shot restore。

### 风险 5：one-shot 失败后不会自动重试

保留下一次 bootstrap / kickstart / 系统重启再次执行；失败日志落到 StandardErrorPath。若要自动重试可用 `KeepAlive` dictionary + `SuccessfulExit=false`，但会增加复杂度并可能反复弹授权/写日志，v1 不启用。

### 风险 6：旧常驻 cleanup-daemon 在升级时收到 SIGTERM 后误 restore

升级安装时先写 stop suppression marker，再 bootout 旧 job；旧进程收到 stop signal 时消费 marker 并跳过 restore。

### 风险 7：one-shot 在 runtime 写入前运行导致误恢复

自动安装任务在 runtime.json 写入且服务 ready 之后触发。真实 launchd 验证包含“服务运行时安装 one-shot，恢复被 skipped”断言。

### 风险 8：状态 API 新增字段导致前端兼容问题

新增字段保持 optional；旧前端可忽略；仅通过 message 表达 needs-upgrade reason 时前端不需要立刻升级。

### 风险 9：没有常驻 LaunchDaemon 后系统关机窗口变短

关机 / 正常 stop 的 restore 责任仍在主进程 signal handler；运行期由 lifecycle helper；系统重启后由 RunAtLoad one-shot。实施后必须验证三条路径。

## 可选增强

- `installed_mode` 字段：已实现（含 serde 与状态返回）。UI 单独徽标 planned, not yet shipped as of 2026-06-17。
- debug-only `BIFROST_SYSTEM_PROXY_LAUNCHD_KEEPALIVE=1` fallback：planned, not yet shipped as of 2026-06-17。
- one-shot outcome 结构化日志字段：`elapsed_ms` 已落地；`outcome` 字段仍为 planned, not yet shipped as of 2026-06-17。

## 回滚方案

若 one-shot 在真实 macOS 上出现不可接受问题：

1. `render_launchd_plist` 恢复写入 `KeepAlive=true`。
2. `run_system_proxy_cleanup_daemon` 恢复长期 signal loop。
3. 将旧 one-shot plist 识别为 needs-upgrade，重新安装回 KeepAlive plist。
4. 保留 lifecycle helper 不变。
5. 通过 `system-proxy launchd install` 或启动自动安装路径重新写入 plist。

回滚后回到当前行为：cleanup-daemon 常驻，RSS 约 5-15 MiB，但兜底语义回旧模型。

## 预期收益

- cleanup LaunchDaemon 空闲 RSS：从约 5-15 MiB 降到 0。
- 空闲 CPU / IO：保持 0。
- 保留启动遗留恢复与运行期崩溃恢复能力。
- 减少用户在活动监视器里看到的 Bifrost 常驻进程数量。

## 推荐推进顺序

1. 实施 one-shot LaunchDaemon 与旧 plist needs-upgrade。
2. 验证 CI、E2E、human_tests 全部通过。
3. 观察真实用户 / 本机 RSS 与清理可靠性。
4. 后续如仍希望把 lifecycle helper RSS 压到极致，再单独设计极小 helper binary。
