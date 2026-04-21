# Remote Invoke 调用取消与终态收尾

> 状态：已实现 | 更新时间：2026-04-21

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
- `crates/bifrost-cli/src/commands/remote.rs` 在 `open_call + subscribe_call_events` 外层监听本地中断信号：
  - Unix：`SIGINT` / `SIGTERM` / `SIGHUP`
  - 其他平台：`ctrl_c`
- 收到中断后立即调用 `cancel_call(call_id, relay_token)`，然后重新订阅 `events` SSE，等待 relay 返回最终 `status=cancelled`
- caller 将 `status=cancelled` 视为终态，返回 exit code `130`

### Client worker 侧取消执行

- `crates/bifrost-admin/src/remote_invoke/worker.rs` 为每个 active call 建立 `ActiveCallControl`
  - 保存 `grant_id`
  - 保存 `started_at`
  - 保存 `cancelled` 标记
  - 保存执行任务 `JoinHandle`
- `call_open` 启动远程执行任务后，把 `JoinHandle` 登记到 `ActiveCallControl`
- 收到 `call_cancel` 时：
  - 先把 `cancelled` 设为 `true`
  - abort 执行任务
  - 无论 `active_calls` 句柄是否仍存在，都尝试更新本地 `call_history` 为 `Cancelled`
  - 从 `active_calls` 中移除

### 终态优先级与竞态处理

- `Cancelled` 是 terminal state
- `Cancelled` 一旦写入，本地晚到的 `Completed` / `Failed` 结果不得覆盖
- 已经 `Completed` / `Failed` / `Timeout` 的调用，再收到迟到的 cancel 也不反向覆盖
- 通过统一的 `update_call_in_history` 状态守卫处理这类竞态
- 如果 `call_cancel` 到达时对应执行任务句柄已经被移除，仍需基于本地历史中的 `started_at` 计算持续时间并写入 `Cancelled`
- 在 SSE 重连或瞬时事件丢失场景下，worker 需要通过 relay 的 `client/calls` 查询结果补做一次终态对账，避免 Recent Calls 长时间停在 `streaming`

### Recent Calls 展示

- Web UI Recent Calls 对以下状态统一映射：
  - `cancelled`：橙色 tag
  - `streaming` / `authorized` / `key_exchanged` / `pending`：processing
  - `completed`：按 `exit_code` 显示绿/红
  - `failed` / `timeout`：红色
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
