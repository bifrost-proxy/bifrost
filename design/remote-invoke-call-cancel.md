# Remote Invoke 调用取消与终态收尾

> 状态：已实现 | 更新时间：2026-06-16

## 背景

`bifrost remote search` 在 caller 侧主动中断后，relay 已经能把调用标记为 `cancelled`，但 caller 和 client 两侧没有把这个终态完整消费：

- caller SSE 未把 `status=cancelled` 当作终态
- client worker 收到 `call_cancel` 后只清理 `active_calls`，没有把本地 `Recent Calls` 历史改成 `cancelled`
- 正在执行中的远程命令没有被真正中断，晚到的完成结果还可能覆盖取消态

这会让 Recent Calls 长时间停在 `streaming`，也会让“用户主动取消”和“真实执行失败”混淆。

## 目标

1. 所有 remote invoke 命令都支持 caller 主动取消，不只限于 `search.get` / `traffic.search`
2. caller 主动取消后，relay、caller、client 三侧统一收敛到 `cancelled`
3. client 侧真正中断正在执行的远程命令，避免继续跑完再覆盖状态
4. Recent Calls 对 `cancelled` 提供明确展示，不再误判为进行中

## 实现方案

### Caller 侧取消协议

- 继续复用 relay 已有的 `POST /v4/remote-invoke/calls/:call_id/cancel`
- `crates/bifrost-cli/src/commands/remote.rs` 中的 `wait_for_remote_call_cancel_signal()` 在 `open_call + subscribe_call_events` 外层监听本地中断信号：
  - Unix：通过 `tokio::signal::unix::signal` 注册 `SIGINT` / `SIGTERM` / `SIGHUP`，并叠加一份 `ctrl_c()` 兜底
  - 非 Unix（Windows 等）：仅监听 `tokio::signal::ctrl_c()`
- `--resume-call-id` 旁路（跳过 `open_call`）和正常路径都在 `tokio::select!` 中并行等待该信号；收到信号后立即调用 `caller.cancel_call(call_id, relay_token)`，随后保持 `subscribe_call_events` SSE 订阅，等待 relay 返回最终 `status=cancelled`
- caller 将 `status=cancelled` 视为终态，返回 exit code `130`

### Client worker 侧取消执行

- `crates/bifrost-admin/src/remote_invoke/worker.rs` 为每个 active call 建立 `ActiveCallControl`
  - 保存 `grant_id`
  - 保存 `started_at`
  - 保存 `cancelled: AtomicBool` 标记
  - `call_info: Mutex<Option<CallInfo>>` 保存当前调用的本地状态快照
  - `task: Mutex<Option<JoinHandle<()>>>` 保存执行任务句柄
  - `stdin_tx: Mutex<Option<mpsc::Sender<Vec<u8>>>>` 保存 PTY/进程 stdin 转发通道（remote shell 复用）
- `call_open` 启动远程执行任务后，把 `JoinHandle` 登记到 `ActiveCallControl.task`
- 收到 `call_cancel` 时（统一收敛在 `apply_cancelled_call`）：
  - 先从 `active_calls` 中 `remove(call_id)`
  - 命中：调用 `mark_cancelled()` 置位 `AtomicBool`，再 `abort_task()` 中断执行任务，最后用 `mark_call_cancelled(duration_ms)` 把本地 `call_info` 推进到 `Cancelled` 并持久化到 `call_history_store`
  - 未命中（句柄已被清理）：直接从 `call_history_store` 取出最近的 `CallInfo`，按本地 `started_at` 计算 `duration_ms` 后写回 `Cancelled`
  - 没有任何本地记录时只打 `debug!` 日志（`cancel reconcile received before call history existed`），不再 panic 或重建

### 终态优先级与竞态处理

- `Cancelled` 是 terminal state
- `Cancelled` 一旦写入，本地晚到的 `Completed` / `Failed` 结果不得覆盖
- 已经 `Completed` / `Failed` / `Timeout` 的调用，再收到迟到的 cancel 也不反向覆盖
- 由 `should_apply_call_result(current, next)` 集中守卫所有状态转移：`ActiveCallControl::update_call_result` 与（仅测试用的）`update_call_in_history` 均通过它判定是否允许覆盖
- 如果 `call_cancel` 到达时对应执行任务句柄已经被移除，仍需基于本地历史中的 `started_at` 计算持续时间并写入 `Cancelled`（由 `apply_cancelled_call` 的回退分支处理）
- 在 SSE 重连或瞬时事件丢失场景下，worker 通过定时器 `active_call_reconcile_ticker`（周期由 `ACTIVE_CALL_RECONCILE_INTERVAL_MS` 控制）调用 `reconcile_active_calls_with_relay()`，对仍登记为 active 的 call 反向查询 relay 的 `client/calls/:id` 接口，按 relay 终态补齐本地 `call_history`，避免 Recent Calls 长时间停在 `streaming`

### Recent Calls 展示

- Web UI `web/src/pages/Settings/tabs/RemoteInvokeTab.tsx` 中的 `getCallStatusColor(call)` 统一映射：
  - `cancelled`：橙色 tag
  - `streaming` / `authorized` / `key_exchanged` / `pending` / `running`：processing
  - `completed`：`exit_code === 0` 显示绿，否则红
  - `failed` / `timeout`：红色
  - 其他：default
- 这样 `cancelled` 不再落到默认色，也不会被误读成仍在执行

## 测试方案

### 单元测试

- `remote.rs`
  - `status=cancelled` 被识别为终态
  - 非终态如 `streaming` 不会被误识别
- `worker.rs`
  - `Cancelled` 写入后不能被晚到的 `Completed` 覆盖
  - 已完成调用不能被迟到的 `Cancelled` 覆盖
  - `call_cancel` 在缺少 `active_call` 句柄时仍能把历史调用写成 `Cancelled`

### E2E 测试

- 更新 `e2e-tests/tests/test_remote_invoke_e2e.sh`
- 新增 caller 主动中断远程调用回归：
  - 触发一个长时间运行的 `remote search`
  - caller 侧发送中断
  - 验证 client 侧 `Recent Calls` 最终为 `cancelled`
  - 验证不会继续停留在 `streaming`
  - 验证取消后的状态在轮询列表中稳定保持 `cancelled`

### 真实场景测试

- 更新 `human_tests/remote-invoke.md`
- 新增 `TC-RI-回归-113 ~ TC-RI-回归-115`
  - `remote search` 被 caller 主动取消后 Recent Calls 显示 `cancelled`
  - `remote status` 被 caller 主动取消后也进入 `cancelled`
  - 取消后晚到结果不覆盖 `cancelled`
- 补充一轮回归，确认本地 relay 下取消态不会因为 worker 重连或事件竞态重新退回 `streaming`

## 校验要求

- 先执行相关 E2E
- 再执行 `cargo test --workspace --all-features`
- 再执行 `cargo fmt --all -- --check`
- 再执行 `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- 最后执行 `rust-project-validate`

## 文档更新要求

- 更新 `human_tests/remote-invoke.md`
- 更新 `human_tests/readme.md`
