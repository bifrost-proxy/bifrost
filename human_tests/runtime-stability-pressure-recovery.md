# Runtime Stability、资源压力降级与系统代理恢复

## 功能模块说明

验证高资源负载下 Bifrost 不因单一 Admin API 超时误杀托管进程，独立健康通道与数据面 canary 仍可判定真实存活；进入资源压力状态后停止 payload 持久化和重任务并保留基础转发；同时验证系统代理恢复策略、ownership generation 和结构化诊断产物。

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
