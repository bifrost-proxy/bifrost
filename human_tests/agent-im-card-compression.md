# Agent IM 卡片压缩次数同步

## 功能模块说明

验证内置 Bifrost Agent 在发生 context compaction 后，飞书 IM progress card 的状态区能从 Agent progress events 中同步最新压缩次数。该用例覆盖 payload 渲染层：`Status`、`ContextUpdated`、`CompactionFinished` 事件进入 `ImAgentProgressSnapshot` 后，卡片 JSON 中的状态标题、footer 和 context/token 指标必须与最新事件一致。

## 前置条件

- 在仓库根目录执行。
- Rust toolchain 可用。
- 本用例不启动 Bifrost 服务、不连接真实飞书，不会修改系统代理。
- 若后续需要验证真实飞书发送链路，必须使用临时 `BIFROST_DATA_DIR` 并启动服务时带 `--no-system-proxy`，除非测试目标明确是系统代理。

## 测试用例列表

### TC-AICC-01：回归 - CompactionFinished 刷新已有飞书卡片 status 压缩次数

**背景**：飞书 progress card 过去只消费 `Status` 事件，忽略 `CompactionFinished`。如果压缩发生后没有立即收到新的 status，卡片可能继续显示旧的 `压缩：0 次`。

**操作步骤**：

1. 运行以下命令：
   ```bash
   cargo test -p bifrost-admin im_gateway::progress_card::tests::progress_snapshot_updates_status_from_compaction_context --lib -- --nocapture
   ```
2. 检查测试输出是否为 `1 passed`。

**预期结果**：

- 测试构造旧 status `compaction_count=0`，再注入 `CompactionFinished` 的 context `compaction_count=2`。
- `ActiveTurnStatus` 被回写为 `compaction_count=2`、`history_version=8`、最新 token/context 指标。
- 生成的飞书卡片 JSON 包含 `压缩：2 次`，不包含 `压缩：0 次`。

**本次执行结果**：通过。2026-05-24 执行 `cargo test -p bifrost-admin im_gateway::progress_card::tests::progress_snapshot_updates_status_from_compaction_context --lib -- --nocapture`，输出 `1 passed`；卡片 JSON 断言包含 `压缩：2 次` 和最新 token 指标，且不包含旧的 `压缩：0 次`。

### TC-AICC-02：回归 - 无 status 时 ContextUpdated 也能渲染压缩次数

**背景**：在卡片刚创建或 status 尚未抵达时，context progress event 仍应让卡片状态区展示压缩次数和 context/token 指标。

**操作步骤**：

1. 运行以下命令：
   ```bash
   cargo test -p bifrost-admin im_gateway::progress_card::tests::progress_snapshot_uses_context_when_status_is_not_available --lib -- --nocapture
   ```
2. 检查测试输出是否为 `1 passed`。

**预期结果**：

- 测试只注入 `ContextUpdated`，不注入 `Status`。
- 状态面板标题显示 `Token：累计 1.1K · 最近 77`。
- footer 显示 `Context：~1.2K / 10K (12.0%)` 和 `压缩：3 次`。

**本次执行结果**：通过。2026-05-24 首轮执行 `cargo test -p bifrost-admin im_gateway::progress_card::tests::progress_snapshot_uses_context_when_status_is_not_available --lib -- --nocapture` 失败，归因为 footer 在 `status=None` 且 `context=Some(...)` 时未渲染 context fallback 指标；修复后复跑同一命令输出 `1 passed`，footer 断言包含 `Context：~1.2K / 10K (12.0%)` 和 `压缩：3 次`。

## 清理步骤

- 本用例不创建持久临时目录，无需额外清理。
- 若命令失败，保留 cargo 输出用于归因；不得削弱断言或删除用例。
