# 系统代理 Reconcile 稳定性真实场景测试

## 功能模块说明

验证已收敛系统代理不会在每个检查周期重复执行接管或重写 manager 附着信息。首次获取、接管或恢复完成后，协调器仍按周期对当前 generation 的所有受管字段执行真实 OS 读回；检查本身不写 OS 配置、不迁移 journal、不改变恢复快照。focused macOS E2E 同时验证一次状态转换、持续只读检查、真实 ownership 与逐服务快照恢复。

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
cargo test -p bifrost-core system_proxy::verification -- --nocapture
cargo test -p bifrost-cli repeated_healthy_inspections_verify_without_repeating_acquire_adopt_or_resume -- --nocapture
```

**预期结果**：

- 获取、接管或恢复后，连续健康检查只验证已附着的 generation，不重复调用获取或接管，也不改写 journal 与原始恢复快照。
- generation 替换、manager 脱离或尚未完成的 journal 不能误入已附着的只读路径；恢复和 retarget 完成后，附着信息必须指向实际 generation。
- 只读检查逐字段验证 HTTP、HTTPS 和受管 bypass；即使聚合状态仍指向 Bifrost，其他受管字段或 service 的变化也必须被发现。同 generation 的字段漂移回到逐字段协调，仅 relinquish 被手动修改的字段，不能覆盖它们；其余受管字段仍可 suspend/resume。
- generation 替换、journal 消失或读回失败不能触发字段协调或重新获取代理。
- 读取失败保留 intent 并允许重试；检查期间出现更高 revision 的 disable，仍按 generation 执行清理。
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
- 已收敛系统代理跨两个 3 秒 reconcile 周期只出现一次 `system proxy transition verified`。
- 同一窗口至少出现两次 `system proxy ownership verified without transition`；脚本开启协调器 debug 日志，避免停止检查的假收敛。
- 脚本成功或失败退出都恢复执行前系统代理状态。
- 每个 network service 的 disabled server/port 也必须与执行前逐字段一致，不能只验证 `Enabled: No`。

### TC-SPRS-04：disabled dormant endpoint 精确恢复回归

**操作步骤**：

```bash
cargo test -p bifrost-core --all-features \
  macos_networksetup_proxy_parser_preserves_disabled_endpoint
```

随后在 macOS CI 的隔离端口执行 `e2e-tests/tests/test_system_proxy_reconcile_stability.sh`，比较脚本记录的 `macos-proxy-before.tsv` 与 `macos-proxy-after.tsv`。

**预期结果**：

- 解析器在 `Enabled: No` 时仍保留 `Server` 与 `Port`，用于恢复 dormant 配置。
- 原 server/port 为空时，停止隔离代理后不能残留 `127.0.0.1:<PROXY_PORT>`。
- 原 server/port 非空但 disabled 时，恢复后值不变且仍为 disabled。

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

## 2026-10-07 收敛修复验证状态

- 当前断言对应 generation 附着与逐字段只读验证实现，不再使用旧的 5 分钟 full reconcile 门槛。
- Rust 格式检查、shell 语法检查、E2E 启动护栏检查、99 项 CI 脚本契约测试和 diff 空白检查通过。
- Linux 插桩验证通过：core 系统代理 174 项、完整 core 1229 项；完整 CLI lib 1432 项、binary 1448 项，包含修正 HTTP 请求分段夹具后的新回归。真实 Linux 压力与 listener handoff E2E 通过。Linux 结果不能替代 macOS manager 路径验证。
- 当前代码的 macOS manager mock、focused E2E 与原生逐服务快照恢复验证待集中执行；本次收敛修改尚未获得原生运行结果。
- 以下 2026-07-31 记录仅描述历史实现，不能作为本次修改已通过验证的证据。

## 2026-07-31 历史执行记录

- `TC-SPRS-01`：通过，core 65、admin 5、CLI reconcile 12×2、收敛决策 1×2 全部成功。
- `TC-SPRS-02`：通过，focused macOS E2E 最新复测 24.4s 完成，两个 3 秒周期内 full reconcile 计数为 1，ownership 保持 `managed_by_bifrost=true`，执行前后逐服务快照一致。
- `TC-SPRS-03`：通过，HTTP/HTTPS/SOCKS 均恢复为 off，18889 无监听残留，共享 9900 仍为 PID 5988。
- 额外诊断：历史全生命周期脚本执行 21 项时 15 项通过、6 项失败，暴露出脚本内部 Desktop ownership 环境泄漏和失败退出恢复不完整；已立即按执行前快照恢复 OS 状态。本次性能门禁使用独立 focused E2E，不削弱 Desktop ownership 保护。
- `TC-SPRS-04`：本地纯解析回归 1/1 PASS；真实空 dormant endpoint 场景由 macOS CI focused E2E 执行，首次运行准确捕获 `Enabled: No` 但 server/port 残留隔离端口的缺陷，修复后结果以同 MR 最新 CI 为准。
