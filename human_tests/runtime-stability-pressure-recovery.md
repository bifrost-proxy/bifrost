# Runtime Stability、资源压力降级与系统代理恢复

## 功能模块说明

验证高资源负载下 Bifrost 不因单一 Admin API 超时误杀托管进程，独立健康通道与数据面 canary 仍可判定真实存活；进入资源压力状态后停止 payload 持久化和主进程内的大型查询，但不按父进程、系统或 worker RSS 拒绝隔离 worker 启动/调用，同时保留基础转发、Replay、AI/IM、ASR、Voice、Remote Invoke 与 Scripts；并验证系统代理恢复策略、ownership generation 和结构化诊断产物。

## 前置条件

- 当前工作目录为 Bifrost 仓库根目录。
- 已构建 debug CLI：`cargo build -p bifrost-cli`。
- 所有运行时验证使用脚本创建的临时数据目录，不修改正式 Bifrost 数据目录或系统代理。

## 测试用例

### TC-RSPR-01：多信号 watchdog 不因 Admin 单点失败误杀

操作步骤：

```bash
cargo test --manifest-path desktop/src-tauri/Cargo.toml desktop_watchdog -- --nocapture
```

预期结果：

- Admin unhealthy，但独立健康 listener 与数据面 canary 正常时，不确认 runtime unresponsive。
- 只有 Admin、数据面均失败，且独立 heartbeat 超时或健康 listener 失败时才确认无响应。
- managed child 明确退出仍走立即恢复路径；旧 runtime marker 没有 `health_port` 时 fail-safe，不误杀。

### TC-RSPR-02：Critical 压力不阻断隔离 worker 与用户操作

操作步骤：

```bash
BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_runtime_pressure_degradation.sh
```

预期结果：

- 独立 loopback health listener 返回 `pressure=critical`，数据面 canary 返回 204。
- Traffic 大查询返回 503，轻量 Admin 请求仍可用。
- 通过代理访问本地 upstream 成功，基础转发未中断。
- Replay Send 通过同一本地 upstream 成功返回 200 和预期响应体，不被通用重任务 guard 误拦截。
- AI/IM provider、Agent config、channel config 与 Remote Invoke 全部管理读取接口均不返回 5xx；Remote Invoke 隔离 Worker 尚未 ready 时使用本地状态回退，不出现全局 `pairings/pending` 503。
- AI 首页及 ASR 页面依赖的 ASR/Voice/Speech 全部读取接口均不返回 5xx；使用真实临时 task 验证 task detail/watch、external import 状态、Daily Agent 配置/指令/记录，以及声纹、唤醒状态路由。
- Scripts 列表、新建/保存和有沙箱上限的 Scripts test 返回 200。
- ASR task 配置创建/修改/暂停/删除、Daily Agent 配置、service stop、Voice listener control、worker jobs 和 AI runner 路由均不返回 pressure 503；真实空目录 task run 能创建并执行 worker job。
- 通过独立 worker 单测验证：即使父进程 pressure=critical，ASR worker 仍完成握手；worker 的高 RSS 不参与前置拒绝。
- 临时服务上的 Playwright 逐页验证 AI Hub/Channels/Agents/Runs、ASR Scheduled/Management/Voice、任务 Overview/Daily/Daily Agent/Records、Remote Invoke 和 Scripts；任一 Admin API 5xx、请求失败或压力错误文案都视为失败。
- Body payload 不写入缓存；doctor 能读到压力状态。

### TC-RSPR-03：恢复策略只允许 fail-open/fail-closed 和 3～5 秒窗口

操作步骤：

```bash
DATA_DIR="$(mktemp -d)"
BIFROST_DATA_DIR="$DATA_DIR" target/debug/bifrost system-proxy recovery-policy fail-open --grace-secs 3
BIFROST_DATA_DIR="$DATA_DIR" target/debug/bifrost system-proxy recovery-policy fail-closed --grace-secs 5
! BIFROST_DATA_DIR="$DATA_DIR" target/debug/bifrost system-proxy recovery-policy fail-open --grace-secs 2
grep -q '^recovery_mode = "fail_closed"$' "$DATA_DIR/config.toml"
grep -q '^recovery_grace_secs = 5$' "$DATA_DIR/config.toml"
rm -rf "$DATA_DIR"
```

预期结果：

- 3 秒 fail-open 与 5 秒 fail-closed 均成功持久化。
- 2 秒窗口被 CLI 参数校验拒绝。
- 最终配置为 `fail_closed`、`recovery_grace_secs=5`。

### TC-RSPR-04：generation 防止恢复流程覆盖外部代理变更

操作步骤：

```bash
cargo test -p bifrost-core guarded_transitions_require_matching_generation_and_observed_owner -- --nocapture
cargo test -p bifrost-core owner_state_updates_atomically_and_events_round_trip -- --nocapture
cargo test -p bifrost-core lifecycle_events_rotate_before_append -- --nocapture
```

预期结果：

- suspend/resume 只有 generation 匹配且当前 OS 现场仍属于预期 owner 时才允许执行。
- stale generation 或外部代理已接管时拒绝写系统代理。
- owner state 原子落盘，结构化 lifecycle events 可读取且有界轮转。

### TC-RSPR-05：已接受的开启意图跨暂停与重启保留

前置条件：仅在允许修改系统代理的隔离 macOS 测试机执行；先记录所有网络服务的代理基线，使用独立 data-dir 和非正式端口。

操作步骤：

1. 执行 `uname -s`。不是 Darwin 或未获得该测试机授权时停止，记录阻塞，不能在日常电脑替代执行。
2. 启动本次构建，开启 System Proxy，记录 `bifrost system-proxy doctor --format json-pretty` 与 Settings 的 configured/effective 状态。
3. 暂停 core 进程使 watchdog 进入故障恢复。观察 fail-open 后 OS 配置回到测试前基线，而 configured enabled 保持 true。
4. 让替换 core 恢复，确认连续健康采样后恢复代理，无需再次点击开关。
5. 在恢复等待中主动关闭代理，确认替换进程及后续采样都不重新开启。

预期结果：临时暂停不改写意图；新关闭优先；无服务长期指向已退出的 Bifrost listener。恢复原配置不等于验证外部网络一定可达。

### TC-RSPR-06：字段级原配置与人工改动

前置条件与平台检查同 TC-RSPR-05。

操作步骤：

1. 在隔离网络服务中准备不同 HTTP/HTTPS 开关和 bypass，并保存完整原值。
2. 开启 Bifrost，再只修改其中一个协议的 OS 设置；关闭 Bifrost。
3. 检查被人工修改字段未被覆盖，其他仍由 Bifrost 管理的字段恢复原值。
4. 重复测试：接管后禁用服务，再执行清理，然后重新启用该服务。
5. 将接管服务重命名，触发恢复并查看 doctor/journal。该场景预期为明确的未完成恢复，而不是错误地宣称完成或猜测另一个服务。

预期结果：v3 快照逐服务/逐字段恢复；独立人工修改被保留；服务禁用不丢失清理责任；名称变化保留未完成记录。测试后手动恢复预先保存的全部基线。

### TC-RSPR-07：授权取消不会重复弹窗

前置条件与平台检查同 TC-RSPR-05；使用确实需要权限的隔离 macOS 配置。

操作步骤：

1. 在 Settings 或 CLI 发起一次明确 enable，在授权对话框中取消。
2. 等待至少两个普通 reconcile 周期，确认同 generation 不再弹出授权请求。
3. 检查 desired 仍为用户选择，effective 状态/错误如实显示未应用，ownership 记录保留授权抑制。
4. 再次明确选择 enable，确认这是一次新请求，允许重新授权；旧对话框的迟到结果不得抑制新 generation。

预期结果：一次取消只终止当前授权尝试；不能自动循环询问，也不能把取消写成关闭偏好。

### TC-RSPR-08：Desktop 替换与停止交接

前置条件与平台检查同 TC-RSPR-05。

操作步骤：

1. 用 Desktop 启动 core 并记录 PID/start identity/端口/generation。
2. 在 watchdog 正在探测时手动重启或切换端口；确认旧采样不会结束新 PID。
3. 模拟一次启动失败后恢复启动条件，确认有界重试最终启动健康实例。
4. 在等待重试时点击停止，确认后续自动任务不再次启动或修改代理。
5. 切换端口，检查旧端口仅在 OS 配置不再引用它之后退役；未完成交接期间不能继续堆叠新端口切换。

预期结果：进程/端口/代理所有权一致；正常关闭和外部 runtime 接管优先；失败和熔断状态不能虚报 ready。

## 执行记录

| 日期 | 用例 | 结果 | 证据摘要 |
| --- | --- | --- | --- |
| 2026-08-22 | TC-RSPR-01 | 通过 | `cargo test --manifest-path desktop/src-tauri/Cargo.toml desktop_watchdog -- --nocapture`：8/8 通过；独立 worktree 首次执行缺少 sidecar 与 `web/dist-desktop`，按正式桌面构建链补齐测试前置后复跑通过。 |
| 2026-08-22 | TC-RSPR-02 | 通过 | `BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_runtime_pressure_degradation.sh`：23 项断言通过；除原有 Critical health、canary、Traffic 503、基础转发、Replay、payload 与 doctor 外，IM providers、AI config/channels、ASR capabilities/status/tasks、speech pipeline status、Remote Invoke status、Scripts list/save 均为 200，Scripts test、新 AI turn 与 ASR service start 仍为 503。随后在独立 Critical 实例上用 Playwright 验证 AI Hub、ASR、Channels、Agents、Runs、Remote Invoke、Scripts 新建保存、Replay Send 共 8 条 WebUI 链路，页面、控制台与网络失败均为 0。 |
| 2026-08-22 | TC-RSPR-03 | 通过 | 临时数据目录中 fail-open 3 秒、fail-closed 5 秒持久化成功；2 秒参数被拒绝；最终配置字段校验通过，目录自动回收。 |
| 2026-08-22 | TC-RSPR-04 | 通过 | generation guard、owner/events 原子落盘与 lifecycle rotation 三个定向单测全部通过。 |
| 2026-08-23 | TC-RSPR-02 | 通过 | 使用隔离构建产物和脚本自动创建的临时 data-dir/动态端口执行：50+ 个 ASR/Voice/Speech、AI/IM、Remote Invoke、worker-jobs、Scripts、Replay API 均未出现 pressure 503/5xx；真实空目录 ASR task 在 `critical` 下成功创建 worker job；Scripts Test、Replay 请求及回放流量详情均为 200；Playwright 遍历 14 个页面且没有 Admin API 4xx/5xx（导航取消除外）或请求失败。另以全新正常压力实例确认 RSS 约 106 MiB 时 health `pressure=normal`。主服务 9900/9901 未操作。 |

| 2026-10-07 | TC-RSPR-05～08 | 阻塞，未执行原生动作 | 平台前置检查实际返回 Linux；本任务禁止修改任何真实主机代理配置，未使用用户 Mac。已停止在平台检查处，未把 fake-OS、headless 或 Darwin source-check 计为原生验证通过。 |
| 2026-10-07 | TC-RSPR-02 shell/API 部分 | 部分验证通过 | 本次 CLI 构建实际运行 `test_runtime_pressure_degradation.sh`，critical 压力下转发、Replay、worker task、API 降级、payload 与 doctor 断言通过；使用临时目录、动态端口和 `--no-system-proxy`。本轮没有复跑该用例的浏览器 UI 部分，也没有运行原生 OS 代理动作。 |

## Non-root CI recovery companion (native execution pending)

- `e2e-tests/tests/test_system_proxy_nonroot_recovery.sh` reuses the guarded, serialized native fixture on an explicitly opted-in disposable GitHub macOS runner. Core and lifecycle helper run as the ordinary non-root runner UID, bound to loopback with isolated data and HOME.
- The deterministic SIGSTOP point is taken only after the real ownership flock is free. This does not prove recovery can bypass a live core frozen while holding that lock; lock contention and partial-operation safety remain separate deterministic tests.
- Product assertions require fail-open routing off, preserved intent and the same active lease, followed by stable automatic native resume without replacing the core. Present endpoints, bypass and out-of-scope SOCKS/PAC/autodiscovery are checked. Unsupported empty dormant metadata is reported explicitly, never counted as exact non-root restoration.
- Only after those assertions, controlled lock-held fixture termination preserves the journal. A separate, narrowly scoped privileged cleanup restores the exact baseline. That cleanup is not evidence of ordinary-user product stop/restoration. The privileged exact-clear companion retains its stricter assertions.
- No native companion execution has been performed by this test-only change; actual results must be recorded from the final candidate's GitHub macOS jobs.
