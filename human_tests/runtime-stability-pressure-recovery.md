# Runtime Stability、资源压力降级与系统代理恢复

## 功能模块说明

验证高资源负载下 Bifrost 不因单一 Admin API 超时误杀托管进程，独立健康通道与数据面 canary 仍可判定真实存活；进入资源压力状态后停止 payload 持久化和新重任务，同时保留基础转发、用户主动发起的单次 Replay，以及 AI/IM、Remote Invoke、Scripts 的配置与状态控制面；并验证系统代理恢复策略、ownership generation 和结构化诊断产物。

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

### TC-RSPR-02：Critical 压力下保留转发并暂停非关键能力

操作步骤：

```bash
BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_runtime_pressure_degradation.sh
```

预期结果：

- 独立 loopback health listener 返回 `pressure=critical`，数据面 canary 返回 204。
- Traffic 大查询返回 503，轻量 Admin 请求仍可用。
- 通过代理访问本地 upstream 成功，基础转发未中断。
- Replay Send 通过同一本地 upstream 成功返回 200 和预期响应体，不被通用重任务 guard 误拦截。
- AI/IM provider、Agent config、channel config 与 Remote Invoke 管理接口均不返回 5xx；Remote Invoke 隔离 Worker 尚未 ready 时，本地状态读取、shell policy 匹配、文件路径校验、SSH Key 创建/修改/重置/撤销和 Recent Calls 清理使用主进程控制面回退，不出现全局 503；discovery、pairing、grant relay 等网络操作仍不在主进程降级执行。
- AI 首页及 ASR 页面依赖的 ASR/Voice/Speech 全部读取接口均不返回 5xx；使用真实临时 task 验证 task detail/watch、external import 状态、Daily Agent 配置/指令/记录，以及声纹、唤醒状态路由。
- Scripts 列表与新建/保存返回 200；Scripts test 和新 AI runner turn 仍返回 503，确认只暂停执行面。
- ASR task 配置创建/修改/暂停/删除、Daily Agent 配置、service stop 和 Voice listener progress/stop 保持可用；模型初始化、service start、转写、task run/resume/import/retry/compress、Daily Agent run 与 Voice session/listener start 仍返回 503。
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

## 执行记录

| 日期 | 用例 | 结果 | 证据摘要 |
| --- | --- | --- | --- |
| 2026-08-22 | TC-RSPR-01 | 通过 | `cargo test --manifest-path desktop/src-tauri/Cargo.toml desktop_watchdog -- --nocapture`：8/8 通过；独立 worktree 首次执行缺少 sidecar 与 `web/dist-desktop`，按正式桌面构建链补齐测试前置后复跑通过。 |
| 2026-08-22 | TC-RSPR-02 | 通过 | `BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_runtime_pressure_degradation.sh` 在动态端口和临时数据目录中通过：逐项覆盖 ASR/Voice/Speech 的读取、配置、停止与重任务拒绝路由；`bifrost im provider list` 经临时 runtime 路由成功且未出现 503；Remote Invoke 子 Worker 在 Critical pressure 下未启动时，status、pending pairings、grants、identity、calls、shell/file-access config、SSH key 读取，以及 shell policy match、path validate、SSH Key create/patch/reset/delete、calls clear 均返回 200；Scripts list/save、IM/AI 配置、基础转发、Replay、payload 降级和 doctor 均符合预期。Playwright 随同临时服务逐页验证 AI Hub/Channels/Agents/Runs、ASR Scheduled/Management/Voice、任务 Overview/Daily/Daily Agent 列表/详情/Records、Remote Invoke 与 Scripts 共 14 个页面，Admin API 5xx、非导航请求失败和压力错误文案均为 0。 |
| 2026-08-22 | TC-RSPR-03 | 通过 | 临时数据目录中 fail-open 3 秒、fail-closed 5 秒持久化成功；2 秒参数被拒绝；最终配置字段校验通过，目录自动回收。 |
| 2026-08-22 | TC-RSPR-04 | 通过 | generation guard、owner/events 原子落盘与 lifecycle rotation 三个定向单测全部通过。 |
