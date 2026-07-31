# 系统代理 Reconcile 稳定性真实场景测试

## 功能模块说明

验证已收敛系统代理不会每 30 秒执行完整全服务检查，Admin OS 操作不占用 Tokio worker；决策与 core 单测继续覆盖 bypass、外部 ownership 和恢复边界，focused macOS E2E 验证真实 ownership 与逐服务快照恢复。

## 前置条件

- 仅在 macOS 执行真实场景。
- 执行前 `scutil --proxy` 的 HTTP/HTTPS/SOCKS 状态已记录。
- `127.0.0.1:18889` 空闲；正式 9900 服务 PID 已记录。
- 使用 `target/debug/bifrost`、隔离数据目录和脚本内 snapshot/cleanup trap。

## 测试用例列表

### TC-SPRS-01：收敛决策、ownership 和 bypass 单元回归

**操作步骤**：

```bash
cargo test -p bifrost-core system_proxy -- --nocapture
cargo test -p bifrost-admin handlers::proxy::tests -- --nocapture
cargo test -p bifrost-cli system_proxy_reconcile -- --nocapture --test-threads=1
cargo test -p bifrost-cli converged_system_proxy_skips_only_high_frequency_full_reconcile -- --nocapture
```

**预期结果**：

- 已收敛且未到 5 分钟完整审计门槛时跳过 full reconcile。
- 到审计门槛、首次接管、disabled、外部 owner 等路径不被错误跳过。
- bypass 比较忽略顺序、大小写和 `,`/`;` 分隔，但真实差异必须重新应用。
- disable 仍保留外部代理，不把它误判成 Bifrost ownership。

### TC-SPRS-02：macOS 真实系统代理 focused 性能与恢复回归

**操作步骤**：

```bash
BIFROST_BIN=target/debug/bifrost \
PROXY_PORT=18889 \
e2e-tests/tests/test_system_proxy_reconcile_stability.sh
```

**预期结果**：

- 隔离代理成功启用系统代理，Admin API 报告 `managed_by_bifrost=true`。
- 已收敛系统代理跨两个 3 秒 reconcile 周期只出现一次 `system proxy full reconcile completed`。
- 脚本成功或失败退出都恢复执行前系统代理状态。

### TC-SPRS-03：正式服务与 OS 状态不受测试残留影响

**操作步骤**：

```bash
scutil --proxy
lsof -nP -iTCP:9900 -sTCP:LISTEN
lsof -nP -iTCP:18889 -sTCP:LISTEN || true
```

**预期结果**：

- HTTP/HTTPS/SOCKS enable 状态与测试前一致。
- 9900 仍由测试前同一 PID 监听。
- 18889 没有测试进程残留。

## 清理步骤

- 如脚本被外部终止，执行其 trap 并根据 snapshot 恢复系统代理。
- 只清理脚本记录的测试 PID 和临时目录，禁止使用 `pkill -f bifrost`。

## 2026-07-31 执行记录

- `TC-SPRS-01`：通过，core 65、admin 5、CLI reconcile 12×2、收敛决策 1×2 全部成功。
- `TC-SPRS-02`：通过，focused macOS E2E 最新复测 24.4s 完成，两个 3 秒周期内 full reconcile 计数为 1，ownership 保持 `managed_by_bifrost=true`，执行前后逐服务快照一致。
- `TC-SPRS-03`：通过，HTTP/HTTPS/SOCKS 均恢复为 off，18889 无监听残留，共享 9900 仍为 PID 5988。
- 额外诊断：历史全生命周期脚本执行 21 项时 15 项通过、6 项失败，暴露出脚本内部 Desktop ownership 环境泄漏和失败退出恢复不完整；已立即按执行前快照恢复 OS 状态。本次性能门禁使用独立 focused E2E，不削弱 Desktop ownership 保护。
