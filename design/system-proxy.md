# System Proxy Ownership

## 功能模块说明

Bifrost 的 System Proxy 只应管理自己写入的系统代理配置。用户同时运行 Surge、Clash、系统级 VPN/代理等外部代理时，Bifrost 可以展示真实系统代理状态，但不能把外部代理误判为自身代理，也不能在 `system-proxy disable`、Admin UI 关闭开关或 `bifrost stop` 时清除外部代理。

## 实现逻辑

- `SystemProxyManager::enable` 在写入系统代理前保存两类状态：`proxy_backup.json` 继续兼容旧恢复逻辑；新增 `proxy_state.json` 记录 Bifrost 本次写入的 target 与写入前 original。
- 关闭路径新增归属判定：只有当前系统代理 host/port 与 Bifrost target 匹配时，才恢复 original 或关闭；如果当前代理指向其他端口或其他 host，则返回 `OwnedByOther` 并保持系统设置不变。
- `bifrost stop` 不再在缺失 runtime 时把任意本机端口代理当作 Bifrost 代理；必须有 runtime host/port 且与当前系统代理匹配才会清理。
- Admin API `GET /api/proxy/system` 返回 `managed_by_bifrost`，WebUI disable 验证在外部代理仍开启但 `managed_by_bifrost=false` 时视为成功，避免误报 `System proxy is still enabled`。
- `bifrost start` 在证书检查和端口冲突检查之前同步执行 `SystemProxyManager::recover_from_crash`。这样电脑重启、睡眠唤醒后旧进程已经消失，或下一次启动因为端口被占用而失败时，也会先恢复 `proxy_state.json` 记录的原始系统代理，避免 Wi-Fi/网络服务继续指向已不存在的 Bifrost 端口。
- `SystemProxyManager::restore` 不再只依赖当前进程内存态 `is_set`；当新进程只看到落盘的 `proxy_state.json` / `proxy_backup.json` 时，也会进入 crash recovery。macOS 恢复如果遇到 `networksetup` 权限错误，沿用 GUI 授权兜底，而不是静默保留残留代理。
- 运行期 system proxy reconcile 不再是一次性启动动作。只要本次服务配置要求启用系统代理，后台线程会周期性复核当前系统代理是否仍指向 Bifrost 端口；macOS 另有 wake-gap reconcile 线程，系统休眠恢复后线程重新调度时如果检测到超过 10 秒的时间跳变，会立即触发一次收敛，不必等待 30 秒周期。该路径只做幂等 enable/reconcile，不做 restore/disable，也不根据调度延迟判断进程异常。
- macOS 启用系统代理时同时启动独立 lifecycle cleanup helper。helper 是单独进程，监听 SIGTERM/SIGINT/SIGHUP 并轮询父进程 PID；主进程被 `kill -9`、崩溃或系统关机导致无法优雅执行 restore 时，helper 会根据 `proxy_state.json` 调用 crash recovery 恢复系统代理。为了避免 CPU 高占用、系统恢复后调度延迟等场景造成误处理，helper 不基于“等待超时”做清理；父 PID 路径必须连续 3 次 poll 都不可见才确认父进程退出。测试可通过 `BIFROST_SYSTEM_PROXY_DISABLE_LIFECYCLE_HELPER=1` 禁用 helper，以保留旧的“下次启动恢复”回归路径。
- 收到 SIGTERM/SIGINT/SIGHUP 时，前台和 daemon 分支都会优先停止系统代理 reconcile 线程并调用 `SystemProxyManager::restore`，再清理代理 listener、后台任务、ASR/浏览器/Agent worker 等资源。前台/daemon listener 异常退出也会先执行同一套 restore，再返回 runtime error；前台兜底 guard 还会在其它异常退出时先停止 reconcile，再执行 restore，避免恢复后被后台线程重新启用。关机/重启前的短窗口优先恢复 Wi-Fi/Web 代理，减少系统重启后仍绑定到已消失 Bifrost 端口的风险。
- macOS 恢复与归属判断必须逐个 network service 检查 `networksetup -getwebproxy` 和 `-getsecurewebproxy`。`scutil --proxy` 是聚合视图，可能出现 Wi-Fi 已恢复但 USB/Thunderbolt service 仍残留 Bifrost 端口的混合状态；当 `proxy_state.json` 中有 Bifrost target 时，restore 只恢复仍指向该 target 的 network service，避免关机时对已被外部代理接管或已恢复的 service 做额外 `networksetup` 调用。只有旧版 `proxy_backup.json` 缺少 target 信息时，才退回全 service 恢复。
- 日志覆盖启动恢复、关机清理、锁等待和 service 级 macOS 写入路径。启动时记录 stale state 检查与 crash recovery 决策；关机信号路径记录停止 reconcile、`waiting_for_system_proxy_lock`、`acquired_system_proxy_lock`、restore 开始、耗时、成功或失败；core 层记录恢复到的原始 proxy host/port、目标 service 选择、是否保留外部代理，以及每个 macOS network service 的设置/关闭动作和耗时，方便重启或休眠恢复后按日志定位清理是否执行、执行到哪一步失败。

## 依赖项

- macOS 使用 `networksetup` / `scutil --proxy` 获取和写入系统代理。
- Windows/Linux 继续复用现有 `sysproxy` / 注册表 / gsettings 路径。
- WebUI 复用 `SystemProxyStatus`，新增字段保持 optional，兼容旧服务端响应。

## 测试方案

- 单元测试：
  - `ProxyBackup::target_matches` 覆盖 loopback host alias 和端口不匹配。
  - `decide_managed_state_recovery` 覆盖当前系统代理仍指向 Bifrost target 时恢复 original、当前代理已指向外部端口时保留外部代理。
  - Admin disable 验证覆盖“外部代理仍启用但不归 Bifrost 管理时视为关闭成功”。
  - CLI stop host 判定覆盖 wildcard listen host 到 loopback 的映射。
- E2E 测试：更新 `e2e-tests/tests/test_system_proxy_e2e.sh`，新增 macOS 外部代理回归：先设置外部本机端口代理，启动 Bifrost `--no-system-proxy`，调用 Admin API disable，断言外部代理仍保留且返回 `managed_by_bifrost=false`；新增 lifecycle helper 崩溃兜底回归：强杀启用系统代理的主进程后，断言 helper 在父进程消失后清理残留；保留 helper 禁用时的崩溃残留回归，确认下次启动失败前也会执行 crash recovery。
- 真实场景测试：更新并执行 `human_tests/cli-system-proxy.md` 的 Surge/外部代理回归用例、睡眠恢复可用用例、lifecycle helper 崩溃兜底用例，以及关机/崩溃残留恢复用例，验证 CLI disable 与 stop 不清理外部代理，睡眠恢复后 Bifrost 仍可处理流量，关机/停止信号先恢复系统代理，helper 可在主进程无优雅退出机会时清理残留，且启动失败前也清理 Bifrost 残留代理。
- 日志验证：重启/休眠类人工测试需检查日志包含 `checking for stale system proxy state before startup`、`System proxy crash recovery check starting`、`system proxy scheduler or wake gap detected; reconciling immediately`、`system proxy lifecycle cleanup helper started`、`system proxy shutdown restore starting; stopping reconcile first`、`waiting_for_system_proxy_lock`、`acquired_system_proxy_lock`、`System proxy restore requested`、`Restoring macOS system proxy to saved original state`、`Selected macOS network services still pointing at Bifrost target for restore`、`Disabling macOS network service web proxies` / `Setting macOS network service proxy to requested target`、service 级 `elapsed_ms` 与 `system proxy shutdown restore completed`，失败时应包含对应 `failed to restore system proxy` 或 `system proxy reconcile failed`。

## Review/Fix/Test 闭环方案

- 第 1 轮：复核用户问题、`git diff`、SystemProxyManager/CLI stop/Admin API/Web store 改动，运行核心单测和系统代理 E2E。
- 第 2 轮：复查第 1 轮修复后 diff、design/human_tests/readme 一致性，复跑受影响单测、E2E 和 human_tests。

## 校验要求

- 必须运行 `cargo test -p bifrost-core system_proxy`、`cargo test -p bifrost-admin proxy::tests`、`cargo test -p bifrost-cli commands::stop::tests`。
- 必须运行 `bash e2e-tests/tests/test_system_proxy_e2e.sh`（macOS/Windows 支持平台；使用临时 `BIFROST_DATA_DIR`；系统代理测试目标明确涉及系统代理，允许省略 `--no-system-proxy`；macOS 覆盖 helper 启用和禁用两种崩溃恢复路径）。
- 收尾前按仓库规则运行 `cargo test --workspace --all-features` 与 `rust-project-validate`。

## 文档更新要求

- 更新 `human_tests/cli-system-proxy.md` 增加外部代理归属回归。
- 更新 `human_tests/readme.md` CLI 系统代理用例数与说明。
