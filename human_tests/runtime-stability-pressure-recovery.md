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

## 执行记录

| 日期 | 用例 | 结果 | 证据摘要 |
| --- | --- | --- | --- |
| 2026-08-22 | TC-RSPR-01 | 通过 | `cargo test --manifest-path desktop/src-tauri/Cargo.toml desktop_watchdog -- --nocapture`：8/8 通过；独立 worktree 首次执行缺少 sidecar 与 `web/dist-desktop`，按正式桌面构建链补齐测试前置后复跑通过。 |
| 2026-08-22 | TC-RSPR-02 | 通过 | `BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_runtime_pressure_degradation.sh`：23 项断言通过；除原有 Critical health、canary、Traffic 503、基础转发、Replay、payload 与 doctor 外，IM providers、AI config/channels、ASR capabilities/status/tasks、speech pipeline status、Remote Invoke status、Scripts list/save 均为 200，Scripts test、新 AI turn 与 ASR service start 仍为 503。随后在独立 Critical 实例上用 Playwright 验证 AI Hub、ASR、Channels、Agents、Runs、Remote Invoke、Scripts 新建保存、Replay Send 共 8 条 WebUI 链路，页面、控制台与网络失败均为 0。 |
| 2026-08-22 | TC-RSPR-03 | 通过 | 临时数据目录中 fail-open 3 秒、fail-closed 5 秒持久化成功；2 秒参数被拒绝；最终配置字段校验通过，目录自动回收。 |
| 2026-08-22 | TC-RSPR-04 | 通过 | generation guard、owner/events 原子落盘与 lifecycle rotation 三个定向单测全部通过。 |
| 2026-08-23 | TC-RSPR-02 | 通过 | 使用隔离构建产物和脚本自动创建的临时 data-dir/动态端口执行：50+ 个 ASR/Voice/Speech、AI/IM、Remote Invoke、worker-jobs、Scripts、Replay API 均未出现 pressure 503/5xx；真实空目录 ASR task 在 `critical` 下成功创建 worker job；Scripts Test、Replay 请求及回放流量详情均为 200；Playwright 遍历 14 个页面且没有 Admin API 4xx/5xx（导航取消除外）或请求失败。另以全新正常压力实例确认 RSS 约 106 MiB 时 health `pressure=normal`。主服务 9900/9901 未操作。 |
