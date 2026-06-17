# System Proxy LaunchDaemon One-Shot Optimization

## 状态

已落地（2026-06-17）。one-shot plist、`SystemProxyLaunchdMode`、`needs_upgrade_reason`、cleanup-daemon 单次退出与 lifecycle helper 兜底均已在 `bifrost-core` / `bifrost-cli` 中实现，并被 E2E 与 human_tests 覆盖。本文档保留为设计纪要；后续行为变更需要新的设计单。

## 背景

当前 macOS system proxy cleanup LaunchDaemon 指向：

```text
bifrost system-proxy cleanup-daemon --data-dir <dir> --installed-version <version>
```

plist 同时设置 `RunAtLoad=true` 和 `KeepAlive=true`。这保证 cleanup daemon 常驻，但它实际复用完整 `bifrost` CLI binary。即使业务逻辑只做启动恢复检查和信号等待，空闲时仍会承担完整二进制、日志、Tokio runtime 和链接依赖的基础 RSS 成本。实测/观察到 5-15 MiB RSS 属于合理范围，但对“只负责系统代理兜底清理”的职责来说不是极致。

本优化目标是把 LaunchDaemon 变为 one-shot：系统加载或手动 kickstart 时执行一次恢复检查，然后退出。空闲时不保留 cleanup-daemon 进程；运行中主进程崩溃仍由已有 lifecycle helper 兜底。

## 决策摘要

推荐实现路径：

1. LaunchDaemon 从常驻模式改为 one-shot 模式。
2. one-shot LaunchDaemon 只负责系统启动、bootstrap、kickstart 后的遗留 system proxy 恢复检查。
3. 运行中的 Bifrost 主进程异常退出继续由 lifecycle helper 负责，不把这部分职责转移给 LaunchDaemon。
4. 正常 stop / SIGTERM / listener exit 继续由主进程 restore，且保持外部代理 ownership 保护。
5. `needs_upgrade` 不应再由版本号单独决定。只要 plist 指向的 Bifrost 程序路径、data dir 和运行模式仍正确，版本号变化不需要重新安装 LaunchDaemon。
6. 旧版 `KeepAlive=true` plist 必须被识别为需要升级，即使 Bifrost 版本号相同。

不推荐路径：

- 不用 `KeepAlive=true` 继续常驻，因为这正是 RSS 消耗来源。
- 不用 `LaunchOnlyOnce=true`，因为它会削弱同一系统启动周期内重新 kickstart 验证和修复的能力。
- 不用 `KeepAlive` dictionary 做失败重试作为默认策略，因为系统代理恢复失败往往涉及权限、系统服务状态或 networksetup 错误，自动反复重启可能制造日志风暴，且不能改善用户取消授权这类场景。
- 不在本阶段拆极小 binary。先用 one-shot 消除 LaunchDaemon 空闲 RSS，再根据 lifecycle helper 的真实 RSS 决定是否进入第二阶段。

## macOS launchd 支持性结论

结论：macOS launchd 支持本方案，不需要额外常驻进程。

依据来自本机 `man launchd.plist`：

- `RunAtLoad` 用于在 job 被加载时启动一次；默认值是 `false`。
- `KeepAlive` 用于让 job 持续运行或按条件重启；默认值是 `false`。
- `KeepAlive=true` 会隐含 `RunAtLoad`，并让退出后的 job 继续被 launchd 管理和重启。

因此 one-shot 模式的 plist 应明确保留：

```xml
<key>RunAtLoad</key>
<true/>
```

并删除 `KeepAlive`，或显式不写 `KeepAlive`。这样 job 在 bootstrap/load 时运行一次，进程退出后不会被 launchd 因 KeepAlive 重新拉起。

不建议使用 `LaunchOnlyOnce=true`。该 key 表示同一次系统启动期间只能运行一次，不适合我们保留 `launchctl kickstart` / 重新 bootstrap 后再次做恢复检查的调试和修复路径。

不使用已废弃/不推荐的 `OnDemand`。

### OS 支持边界

- 目标平台：macOS system LaunchDaemon，路径为 `/Library/LaunchDaemons/com.bifrost.system-proxy-cleanup.plist`。
- 依赖命令：`/bin/launchctl`、`/usr/bin/osascript`、`networksetup`。
- 非 macOS：保持 no-op / unsupported 语义，不新增 one-shot 行为。
- macOS 上 `RunAtLoad` 是 launchd.plist 标准 key。即使文档建议避免无必要的 speculative job launches，本场景只在用户启用 system proxy 后安装，并且 one-shot 退出，资源影响小于当前 KeepAlive 常驻。
- system domain LaunchDaemon 不依赖用户登录 session；GUI 授权只发生在安装/卸载时，one-shot 执行时不应弹 GUI 授权。

## 目标行为

### 开机或 LaunchDaemon 加载

1. macOS 启动、`launchctl bootstrap system <plist>` 或 `launchctl kickstart system/com.bifrost.system-proxy-cleanup` 触发 one-shot cleanup command。
2. cleanup command 执行 `recover_if_no_live_runtime(data_dir)`。
3. 如果 `runtime.json` / `bifrost.pid` 仍指向 live Bifrost runtime，直接退出，不清理系统代理。
4. 如果 managed proxy target 仍有 live listener，直接退出，不清理系统代理。
5. 如果 Bifrost runtime 不存在，且 `proxy_state.json` 显示系统代理仍由 Bifrost 管理，则恢复 original proxy 或关闭 Bifrost 写入的 system proxy。
6. cleanup command 无论恢复成功、跳过或记录失败，都退出；空闲状态不保留 cleanup-daemon 进程。

### Bifrost 运行期崩溃

运行中异常退出继续由现有 lifecycle helper 覆盖：

1. Bifrost 主进程启用 system proxy 后启动 `system-proxy lifecycle-helper --parent-pid <pid>`。
2. helper 监听 SIGTERM/SIGINT/SIGHUP，并以 2 秒间隔轮询父进程。
3. 父进程连续 3 次不可见才判定异常退出，执行 `SystemProxyManager::recover_from_crash(data_dir)`。
4. 该路径不依赖 LaunchDaemon 常驻。

### 正常停止

正常 `bifrost stop` / SIGTERM / listener exit 仍由主进程先停止 reconcile，再执行 `SystemProxyManager::restore`。如果当前系统代理已被外部代理接管，仍按 ownership 判断保留外部代理，不误清理。

## 覆盖矩阵

| 场景 | 预期行为 | 负责组件 | 验证方式 |
| --- | --- | --- | --- |
| 用户开机后上次 Bifrost 残留 system proxy | one-shot 恢复 original proxy 或关闭 Bifrost proxy | LaunchDaemon one-shot | `launchctl print`、日志、`system-proxy status` |
| 安装 LaunchDaemon 时 Bifrost 主服务正在运行 | one-shot 检查到 live runtime 后跳过并退出 | LaunchDaemon one-shot | 安装后日志包含 startup recovery skipped，2 秒后无 cleanup-daemon 进程 |
| 运行中主进程 `kill -9` | lifecycle helper 连续 3 次 miss 后恢复 system proxy | lifecycle helper | E2E/human_test 强杀主进程 |
| 正常 `bifrost stop` | 主进程优雅 restore，helper/LaunchDaemon 不重复误清 | 主进程 | CLI stop 和日志断言 |
| 当前 system proxy 已被外部代理接管 | 所有清理路径保留外部代理 | SystemProxyManager ownership | 外部代理归属回归 |
| 用户取消 LaunchDaemon 安装授权 | system proxy 可以开启，但系统启动兜底缺失 | Admin/CLI install path | UI/API 状态提示和日志 |
| Bifrost 升级但二进制路径和 data dir 不变 | 不重装 LaunchDaemon，只展示 installed/current version 差异 | launchd status | 单测和真实状态检查 |
| 旧版 KeepAlive plist 已安装 | 标记 needs_upgrade，重新安装 one-shot plist | launchd status / install | 单测和真实 launchctl status |
| one-shot 恢复失败 | 记录失败并退出，不循环重启 | cleanup-daemon | 日志和退出码断言 |

## 非目标

- 本阶段不拆独立极小 helper binary。
- 本阶段不改变 system proxy ownership、original proxy 恢复语义、外部代理保留语义。
- 本阶段不移除 lifecycle helper。
- 本阶段不改变 GUI 授权安装流程，只改变安装后的 plist 持久运行模式和 cleanup-daemon 退出行为。
- 本阶段不改变 start 时的异步 LaunchDaemon 自动安装策略，包括用户取消授权不阻塞主服务。
- 本阶段不解决所有 system proxy 恢复失败的根因，只改变守护方式和资源占用。

## 设计方案

### 1. plist 渲染

修改 `render_launchd_plist`：

- 保留 `RunAtLoad=true`。
- 移除 `KeepAlive=true`。
- 不写 `OnDemand`、`LaunchOnlyOnce`、`SuccessfulExit`。
- 保留 `ProgramArguments`、`StandardOutPath`、`StandardErrorPath` 和 `BIFROST_LAUNCHD_INSTALLED_VERSION`。
- 可选新增环境变量 `BIFROST_LAUNCHD_MODE=oneshot`，便于日志和未来诊断（planned, not yet shipped as of 2026-06-17；当前实现仅写入 `BIFROST_LAUNCHD_INSTALLED_VERSION`）。

预期片段：

```xml
<key>RunAtLoad</key>
<true/>
```

不再出现：

```xml
<key>KeepAlive</key>
<true/>
```

### 2. cleanup-daemon 入口

修改 `run_system_proxy_cleanup_daemon`：

- 默认执行 one-shot startup recovery 后立即返回。
- 日志区分 `completed`、`skipped`、`failed`，并新增退出日志，例如 `system proxy launchd cleanup daemon exiting after one-shot recovery`。
- 不再创建长期 Tokio runtime，也不再注册 SIGTERM/SIGINT/SIGHUP 后进入 3600 秒 heartbeat loop。
- `set_data_dir(data_dir.clone())` 仍需保留，确保后续读取 runtime/proxy_state 使用同一 data dir。
- `installed_version` 继续记录到日志，用于定位用户机器上的旧 plist 或旧二进制。

实际实现把 one-shot 主逻辑保留在 `bifrost-core::system_proxy_launchd::recover_if_no_live_runtime_with_startup_retry(data_dir) -> Result<SystemProxyLaunchdRecoveryOutcome>`，返回 `Recovered` / `Skipped` 两种结果（live runtime 与 live proxy target 命中都合并为 `Skipped`，失败通过 `Err` 上抛并在 CLI 入口 `warn!` 记录）。CLI 侧 `run_system_proxy_cleanup_daemon` 直接消费该函数，无需新增独立函数名（细分为 `SkippedLiveRuntime` / `SkippedLiveProxyTarget` / `Failed` 的变体为 planned, not yet shipped as of 2026-06-17）。

退出码建议：

- `0`：恢复成功或安全跳过。
- `0`：恢复失败但错误已记录。理由是 launchd 不应把权限/系统状态类失败解释为需要自动重启。
- 后续如果要引入失败重试，再重新评估是否对不可恢复错误返回非 0。

兼容策略：

- 可保留隐藏参数 `--watch-signals` 或环境变量 `BIFROST_SYSTEM_PROXY_LAUNCHD_KEEPALIVE=1` 作为 debug fallback，但默认不启用。
- 如果保留 fallback，plist 默认仍不写该参数，避免用户安装后常驻。

推荐最小实现：不新增 fallback 参数，直接 one-shot。若后续线上发现 launchd 行为差异，再用版本升级恢复。

### 3. launchd 状态语义

当前 `LaunchDaemon loaded` 不应被解释为“cleanup-daemon 进程正在运行”。one-shot 后正常状态是：

- plist installed: true
- launchd job loaded/bootstrap 成功: true
- cleanup-daemon process: not running
- needs_upgrade: false

需要检查 `launchd_status_for_config` 和 Web UI/CLI 展示文案：

- `Loaded` 继续表示 launchd job 已被 bootstrap，不表示进程常驻。
- 如果 UI 有“running”或“daemon alive”暗示，需要改成“Installed / Loaded / Current”。
- 不把 `launchctl print` 中没有 PID 当作异常。
- status 的 `loaded=true` 只说明 job 已被 bootstrap 到 launchd system domain；one-shot 退出后仍应保持 true。
- 如果 `launchctl print system/<label>` 返回失败但 plist 存在，继续显示 `installed=true loaded=false`，并保留现有 message。

### 4. 安装与升级

安装流程保持：

1. 写入 plist。
2. `launchctl bootout system <plist>` 尝试卸载旧 job。
3. `launchctl bootstrap system <plist>` 加载新 job。
4. `launchctl enable system/<label>`。
5. 可保留 `launchctl kickstart -k system/<label>`，用于安装后立即跑一次 one-shot 检查。

one-shot 安装时如果 Bifrost 主服务仍在运行，`recover_if_no_live_runtime` 应跳过恢复并退出。因此启动时异步安装不会误清正在使用的 system proxy。

升级检测应基于“是否仍指向正确可执行对象和正确运行模式”，而不是单独基于版本号：

- 旧 plist 含 `KeepAlive=true` 时，重新安装当前版本后应写入 one-shot plist。
- Bifrost 版本升级但安装路径不变时，ProgramArguments 里的 program 仍指向同一个 `bifrost` 可执行文件。此时 launchd 再次运行 cleanup command 会直接执行新版本二进制，不需要重新安装 plist。
- 因此 `installed_version` 只作为诊断字段和历史安装版本展示，不应单独触发 `needs_upgrade` 或 GUI 授权重装。
- 即使版本号相同，也需要考虑 plist 内容模式变化。建议引入 `CURRENT_VERSION` 之外的 plist schema/mode 检测，或让 `needs_upgrade` 在解析到旧 `KeepAlive=true` 时返回 true。

实际落地的数据模型（`crates/bifrost-core/src/system_proxy_launchd.rs`）：

```text
SystemProxyLaunchdStatus {
  installed_mode: Option<SystemProxyLaunchdMode>,
  needs_upgrade: bool,
  needs_upgrade_reason: Option<String>,
  ..
}

SystemProxyLaunchdMode = OneShot | KeepAlive | Unknown   // serde rename_all snake_case
```

`parse_installed_plist` 已识别 `keep_alive` / `run_at_load`，`launchd_needs_upgrade_reason` 给出可读升级原因。

升级判定规则：

| 条件 | needs_upgrade |
| --- | --- |
| plist 不存在 | false，状态为 not installed |
| program 不匹配 | true |
| data_dir 不匹配 | true |
| installed_version 不匹配，但 program/data_dir/mode 均匹配 | false，仅展示版本差异 |
| installed_version 不匹配，且 program/data_dir/mode 任一不匹配 | true，由对应结构性不匹配触发 |
| KeepAlive=true | true |
| 缺少 RunAtLoad=true | true |
| plist 解析失败 | true，并显示 Unknown |

现有实现需要调整：当前 `launchd_status_with_expected` 将 `installed_version.as_deref() != Some(CURRENT_VERSION)` 直接纳入 `needs_upgrade`。实施本方案时必须删除这个 version-only 条件，改成：

```text
needs_upgrade = installed && (
  program_mismatch ||
  data_dir_mismatch ||
  launchd_mode_mismatch ||
  missing_run_at_load ||
  plist_parse_failed
)
```

`installed_version` 和 `current_version` 仍保留在状态响应里，帮助排查用户机器上究竟是哪次安装写入的 plist。

### 5. 生命周期职责分工

| 场景 | 负责路径 | 说明 |
| --- | --- | --- |
| 系统启动后发现上次残留 Bifrost system proxy | one-shot LaunchDaemon | `RunAtLoad` 执行一次恢复检查后退出 |
| 安装/升级 LaunchDaemon 后即时检查 | one-shot LaunchDaemon | `bootstrap` / `kickstart` 触发，主服务活着时跳过 |
| Bifrost 主进程 `kill -9` / panic / 被系统杀死 | lifecycle helper | 父 PID 连续 3 次不可见后恢复 |
| 正常 stop / SIGTERM | 主进程 graceful restore | 先停 reconcile，再 restore |
| 外部代理接管 system proxy | ownership 判断 | 所有恢复/关闭路径都不得误清理 |
| 用户取消 LaunchDaemon 安装授权 | 主服务继续运行 | system proxy 可用，但系统重启兜底能力缺失，UI/日志提示 |

### 6. 并发与竞态处理

one-shot 与主进程、lifecycle helper 可能在短时间内同时观察同一 data dir。必须保持以下约束：

- one-shot 先调用 `recover_if_no_live_runtime`，该函数已经先检查 runtime pid，再检查 managed proxy target。不要绕过这两个 guard。
- 主进程 graceful restore 与 lifecycle helper restore 继续依赖 `SystemProxyManager` 的 ownership 判断，避免重复 restore 破坏外部代理。
- 安装/卸载时已有 stop suppression marker 用于防止 launchd stop 触发恢复。one-shot 后该 marker 仍可保留，用于 `bootout` 旧 KeepAlive job 时避免旧进程收到 SIGTERM 后 restore。
- `kickstart -k` 可能杀掉正在运行的旧 KeepAlive cleanup-daemon。安装流程需要继续先写 suppression marker，再 bootout/bootstrap/kickstart。
- one-shot 命令可能在主服务刚启动但 runtime.json 尚未写入时运行。安装任务当前在服务 ready 并写 runtime 后触发，必须保持这个顺序；不要把 LaunchDaemon 自动安装提前到 runtime 写入前。

### 7. 可观测性

必须保留或新增以下日志：

| 日志 | 场景 |
| --- | --- |
| `system proxy launchd cleanup daemon started` | one-shot 入口启动 |
| `system proxy launchd cleanup daemon startup recovery completed` | 发生恢复 |
| `system proxy launchd cleanup daemon startup recovery skipped` | live runtime 或 live proxy target |
| `system proxy launchd cleanup daemon startup recovery failed` | 恢复失败 |
| `system proxy launchd cleanup daemon exiting after one-shot recovery` | one-shot 退出 |
| `system proxy lifecycle cleanup helper started` | 运行期崩溃保护仍启用 |
| `system proxy lifecycle helper confirmed parent exit` | helper 触发恢复 |

建议在 one-shot 退出日志中写入：

- `data_dir`
- `installed_version`
- `current_version`
- `outcome`
- `elapsed_ms`

### 8. 用户体验

- Web UI `Boot/Shutdown Cleanup` 开关语义保持：表示“已安装系统启动/异常兜底保护”，不表示后台有常驻 daemon 进程。
- 文案避免 `running`，改用 `installed`、`loaded`、`current`、`one-shot`。
- 如果旧 KeepAlive plist 被识别为 needs-upgrade，Web UI 应提示需要更新 helper，而不是显示错误。
- 用户取消授权后，system proxy enable 不回滚；但状态区应能说明 cleanup protection 未安装。

## 实施步骤

1. 更新 `crates/bifrost-core/src/system_proxy_launchd.rs`
   - 移除 plist 中的 `KeepAlive`。
   - 扩展 plist 解析，识别旧 keepalive mode。
   - 如需要，增加 `installed_mode` / `needs_upgrade` 判定，确保旧常驻 plist 会被升级为 one-shot plist。
   - 更新 `launchd_plist_contains_cleanup_daemon_version_and_paths` 断言。
   - 新增旧 plist mode/upgrade 判定单测。

2. 更新 `crates/bifrost-cli/src/commands/system_proxy.rs`
   - 将 `run_system_proxy_cleanup_daemon` 改为 one-shot。
   - 删除长期 signal loop 和 heartbeat。
   - 保留启动恢复、跳过、失败日志。
   - 抽出可测试 helper 或通过 CLI E2E 验证命令会退出。
   - 保留 lifecycle helper 代码不变。

3. 更新状态展示
   - CLI `system-proxy launchd status` 文案确认 `Loaded` 不暗示 running。
   - Web UI `Boot/Shutdown Cleanup` 文案如有“daemon running”暗示则改为“installed and loaded”。
   - 如新增 `installed_mode`，同步 OpenAPI、TypeScript 类型和 UI 展示。

4. 更新测试
   - 单元测试断言 plist 包含 `RunAtLoad` 且不包含 `KeepAlive`。
   - 单元测试覆盖旧 keepalive plist 被识别为 needs-upgrade。
   - 单元测试覆盖 cleanup-daemon one-shot 在 recover skipped/completed 后返回。
   - E2E dry-run plist 测试更新断言。
   - human_tests 增加 one-shot 行为验证。
   - 保留并复跑 lifecycle helper 崩溃兜底测试，证明去掉 KeepAlive 不影响运行期异常恢复。

5. 更新文档
   - 更新 `design/system-proxy.md` 中 LaunchDaemon 常驻描述。
   - 更新 `human_tests/cli-system-proxy.md` TC-CSP-16/17/18 预期。
   - 更新 `human_tests/readme.md` 用例说明。

## 详细任务清单

### 必须实现

- plist 默认 one-shot：`RunAtLoad=true`，不写 `KeepAlive`。
- cleanup-daemon 默认执行一次恢复检查并退出。
- 旧 KeepAlive plist 被判定为 needs-upgrade。
- Bifrost 版本变化但 program/data_dir/mode 均匹配时不触发 needs-upgrade。
- status / Web UI 不把无 PID 视为异常。
- lifecycle helper 保持启用，运行期崩溃保护不回退。

### 必须不破坏

- 用户取消 LaunchDaemon 授权不阻塞主服务。
- 启动时系统代理开启后仍自动检查/安装 LaunchDaemon。
- 运行中 Admin API/Web UI 开启 system proxy 后仍自动检查/安装 LaunchDaemon，并启动 lifecycle helper 覆盖本 session 后续崩溃。
- 外部代理 ownership 保护不变。
- 正常 stop / SIGTERM / listener exit restore 顺序不变。
- 下次启动前 crash recovery 不变。

### 必须真实验证

- macOS launchd 真实 install / status / kickstart / process absence。
- 主进程 `kill -9` 后 lifecycle helper 恢复 system proxy。
- 旧 KeepAlive plist 升级为 one-shot plist。
- 版本号不一致但路径/data dir/one-shot mode 一致时不重新安装。
- Web UI / Admin API 开启 system proxy 后自动安装 one-shot LaunchDaemon。

### 可选增强

- `installed_mode` 字段：已实现（含 serde 与状态返回）。UI 展示沿用 message/needs_upgrade_reason，未单独渲染 mode 徽标（planned, not yet shipped as of 2026-06-17）。
- debug-only KeepAlive fallback：planned, not yet shipped as of 2026-06-17。
- one-shot outcome 结构化日志字段：已落地 `elapsed_ms`，`outcome` 字段仍为 planned, not yet shipped as of 2026-06-17。

## 验证计划

### 单元测试

必须执行：

```bash
cargo test -p bifrost-core system_proxy_launchd
cargo test -p bifrost-cli system_proxy
cargo test -p bifrost-admin proxy::tests
```

新增/调整断言：

- `launchd_plist_contains_cleanup_daemon_version_and_paths` 同时断言 `RunAtLoad` 存在、`KeepAlive` 不存在。
- 新增 `keepalive_plist_requires_upgrade_to_oneshot`。
- 新增 `version_mismatch_with_same_program_data_dir_and_mode_does_not_require_upgrade`。
- 新增 `missing_run_at_load_requires_upgrade`。
- 新增 `oneshot_plist_reports_current_mode`。
- 新增 `cleanup_daemon_exits_after_startup_recovery_skipped` 或等价可测试 helper，避免直接依赖真实 launchd。
- 如果新增 `installed_mode`，补充 serde/API schema 测试或 handler snapshot 断言。

### E2E

必须执行：

```bash
BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_system_proxy_e2e.sh
```

重点验证：

- plist dry-run 不含 `KeepAlive`。
- plist dry-run 含 `RunAtLoad`。
- `system-proxy cleanup-daemon` one-shot 后命令退出。
- lifecycle helper 崩溃兜底仍通过。
- `BIFROST_SYSTEM_PROXY_DISABLE_LIFECYCLE_HELPER=1` 的“下次启动恢复”回归仍通过。
- 旧 KeepAlive plist 被 status 标记为 needs-upgrade。
- 已安装 one-shot 且 program/data_dir/mode 匹配时不重复弹授权，即使 installed_version 与 current_version 不一致。

### human_tests

更新并真实执行 `human_tests/cli-system-proxy.md`：

- TC-CSP-16：启动后异步安装 LaunchDaemon，授权后 `launchctl print system/com.bifrost.system-proxy-cleanup` 成功，但 `pgrep -fl "system-proxy cleanup-daemon"` 在短时间后不应存在常驻进程。
- TC-CSP-17：Web UI 安装/卸载 one-shot LaunchDaemon；program/data_dir/mode 一致时不重复弹授权；旧 KeepAlive plist 被识别为 needs-upgrade。
- TC-CSP-18：运行中服务通过 Admin API/Web UI 打开 system proxy 后自动检查/安装 one-shot LaunchDaemon，并验证日志包含 `system proxy lifecycle helper started after Admin API enable`。
- 崩溃兜底：主进程 `kill -9` 后 lifecycle helper 仍在 2 秒 poll / 3 次 miss 后恢复 system proxy。

执行记录必须包含：

- 测试数据目录。
- Bifrost 端口。
- `launchctl print` 结果摘要。
- `pgrep -fl "system-proxy cleanup-daemon"` 安装后和 kickstart 后的结果。
- system proxy enable/disable 前后的 host/port。
- 临时数据目录和 LaunchDaemon 清理结果。

### launchd 真实验证

必须在 macOS 上真实执行：

```bash
./target/debug/bifrost system-proxy launchd install --data-dir "$TEST_DATA_DIR"
launchctl print system/com.bifrost.system-proxy-cleanup
pgrep -fl "system-proxy cleanup-daemon" || true
launchctl kickstart -k system/com.bifrost.system-proxy-cleanup
sleep 2
pgrep -fl "system-proxy cleanup-daemon" || true
```

预期：

- `launchctl print` 能找到 job。
- kickstart 后日志显示 one-shot recovery completed/skipped/failed。
- 2 秒后没有常驻 cleanup-daemon 进程。
- 如果 Bifrost 主服务仍在运行，日志显示 startup recovery skipped。

### 资源占用验证

必须记录 before/after：

```bash
ps -axo pid,rss,vsz,command | rg "system-proxy cleanup-daemon|system-proxy lifecycle-helper"
```

预期：

- LaunchDaemon one-shot 空闲时没有 `system-proxy cleanup-daemon` 进程。
- lifecycle helper 只在 system proxy enabled 且主服务运行时存在。
- cleanup-daemon RSS 从可见的 5-15 MiB 常驻变为 0 常驻。
- CPU/IO 无新增常驻消耗。

### workspace 校验

实现完成后按仓库要求执行：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

最后执行 rust-project-validate。

## 风险与缓解

### 风险 1：去掉 KeepAlive 后无法覆盖运行期崩溃

缓解：运行期崩溃本来由 lifecycle helper 覆盖。实施时必须复跑 helper 崩溃兜底 E2E 和 human_test。

### 风险 1.1：运行中才通过 Admin API/Web UI 打开 system proxy 时没有 lifecycle helper

缓解：Admin API/Web UI 的 enable 成功路径必须调用共享 lifecycle helper state 的 `ensure_started()`；启动时未启用 system proxy 的进程也要在 `AdminState` 中持有该 state。E2E 增加运行中 enable 后的 helper 启动日志断言，human_tests 在 TC-CSP-18 中要求验证该日志。

### 风险 2：launchd status 被误判为 unloaded

缓解：状态判断只依赖 `launchctl print system/<label>` / plist 存在 / 版本匹配，不依赖 cleanup-daemon PID 常驻。

### 风险 3：旧 KeepAlive plist 不会自动升级

缓解：解析 plist 时识别 `KeepAlive=true`，将其计入 `needs_upgrade`，触发重新安装。

### 风险 4：系统关机时没有常驻 LaunchDaemon 收 SIGTERM

缓解：正常关机/stop 由主进程 restore；运行中崩溃由 lifecycle helper restore；系统重启后的遗留状态由 RunAtLoad one-shot restore。该职责拆分覆盖原有目标，同时消除空闲常驻 RSS。

### 风险 5：one-shot 失败后不会自动重试

缓解：保留下一次 bootstrap/kickstart/系统重启再次执行；失败日志落到 StandardErrorPath。若 review 认为必须自动重试，可改为 `KeepAlive` dictionary + `SuccessfulExit=false` 只重启失败退出，但这会增加复杂度，且失败场景可能反复弹授权/写日志。本方案默认不自动重试，避免资源和用户体验副作用。

### 风险 6：旧常驻 cleanup-daemon 在升级时收到 SIGTERM 后误 restore

缓解：保留安装流程中的 stop suppression marker。升级安装时先创建 marker，再 bootout 旧 job，旧进程收到 stop signal 时消费 marker 并跳过 restore。

### 风险 7：one-shot 在 runtime 写入前运行导致误恢复

缓解：保持自动安装任务在 runtime.json 写入和服务 ready 之后触发。真实 launchd 安装验证必须包含“服务正在运行时安装 one-shot，恢复被 skipped”的断言。

### 风险 8：状态 API 新增字段导致前端兼容问题

缓解：新增字段保持 optional，旧前端可忽略；如不需要 UI 展示，先只用 message 表达 needs-upgrade reason，避免扩大 API 面。

### 风险 9：没有常驻 LaunchDaemon 后系统关机窗口变短

缓解：关机/正常 stop 的主要 restore 责任仍在主进程 signal handler；运行期异常由 lifecycle helper；系统重启后的遗留由 RunAtLoad one-shot。实施后必须验证三条路径，而不是只验证 one-shot。

## 回滚方案

如果 one-shot 在真实 macOS 上发现不可接受的问题：

1. 将 `render_launchd_plist` 恢复写入 `KeepAlive=true`。
2. 将 `run_system_proxy_cleanup_daemon` 恢复长期 signal loop。
3. 将旧 one-shot plist 识别为 needs-upgrade，重新安装回 KeepAlive plist。
4. 保留 lifecycle helper 不变。
5. 通过 `system-proxy launchd install` 或启动自动安装路径重新写入 plist。

回滚后预期回到当前行为：cleanup-daemon 常驻，RSS 约 5-15 MiB，但系统启动/关机兜底语义恢复到旧模型。

## Review 门禁

实现前 review 必须确认：

- one-shot LaunchDaemon 职责只覆盖系统启动/bootstrap/kickstart 遗留恢复。
- lifecycle helper 职责覆盖运行期主进程异常退出。
- 正常 stop 和 listener exit 仍由主进程 restore。
- 旧 KeepAlive plist 会自动升级。
- 状态展示不依赖 cleanup-daemon PID 常驻。
- human_tests 包含真实 macOS launchd 和 process absence 验证。
- 如果新增 API 字段，前端和 OpenAPI 同步。

## 预期收益

- cleanup LaunchDaemon 空闲 RSS：从约 5-15 MiB 降到 0。
- 空闲 CPU：保持 0。
- 空闲 IO：保持 0。
- 保留启动遗留恢复和运行期崩溃恢复能力。
- 减少用户在活动监视器里看到的 Bifrost 常驻进程数量。

## 推荐推进顺序

1. 先实施 one-shot LaunchDaemon 和旧 plist needs-upgrade。
2. 验证 CI、E2E、human_tests 全部通过。
3. 观察真实用户/本机 RSS 和清理可靠性。
4. 如果仍希望把 lifecycle helper 的 RSS 也压到极致，再单独设计极小 helper binary。
