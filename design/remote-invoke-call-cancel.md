# Remote Invoke 调用取消与终态收尾

## 背景

`bifrost remote search` / `remote status` / `remote traffic list` / `remote traffic get` 等 remote invoke 命令在执行期间会长时间维持一个 caller → relay → target client 的三段状态机。调用发起 (`open_call`)、事件流 (`subscribe_call_events`)、终态汇报 (`client/exit`)、caller 取消 (`calls/:id/cancel`) 都要跨越 relay 才能收敛。

历史实现里，caller 侧的 `Ctrl-C` 只被 `search.get` / `traffic.search` 少数命令识别；relay 已经把调用标记为 `cancelled`，但 client worker 收到 `call_cancel` 后只清理 `active_calls`，没有把本地 `Recent Calls` 历史改成 `cancelled`；正在执行中的远程命令没有被真正中断，晚到的完成结果还会覆盖取消态。结果是 Recent Calls 长时间停在 `streaming`，"用户主动取消"和"真实执行失败"混淆，客服和自动化 agent 都无法判定应该重试还是升级。

本方案把取消能力提升为 remote invoke 通用能力：任意 remote 命令都能通过本地信号触发取消，relay/caller/client 三侧统一收敛到 `Cancelled` 终态，client 侧对正在跑的进程做真正的 abort，晚到事件不能反向覆盖终态。

## 用户目标验证清单

### 必须实现

- 所有 remote invoke 命令（`search`、`status`、`traffic list/get`、`file.*`、`shell.exec` 等）均支持 caller 主动取消。
- caller 侧收到 `SIGINT` / `SIGTERM` / `SIGHUP`（Unix）或 `Ctrl-C`（Windows）后必须立刻发出 `POST /v4/remote-invoke/calls/:call_id/cancel`。
- caller 在 relay 返回 429 / 事件流丢失时仍能在有限时间内收敛到 `cancelled` 并退出，不会无限挂起。
- caller 对 `status=cancelled` 视为终态，进程返回 exit code `130`。
- client worker 收到 `call_cancel` 后必须 abort 正在跑的执行任务，并把 `call_history` 中对应记录写成 `Cancelled`。
- `Cancelled` 一旦写入本地历史，晚到的 `Completed` / `Failed` / `Timeout` 结果不能覆盖它。
- 已经 `Completed` / `Failed` / `Timeout` 的调用，晚到 `Cancelled` 也不允许反向覆盖。
- SSE 重连或事件丢失场景，worker 通过周期性 reconcile 补齐 relay 侧终态。
- Web UI Recent Calls tab 对 `cancelled` 有可辨识的展示颜色，不再被误认为 processing。

### 必须不破坏

- 正常成功路径 (`streaming → completed`) 保持不变。
- `--resume-call-id` 断线重连能力保持工作。
- Cancel 不发出 `POST … /cancel` 之外的副作用（不改动 grant、不写额外历史行）。
- 已有 `Recent Calls` 分页 / 过滤 / 详情 API 语义保持不变。

### 必须真实验证

- E2E：`test_remote_invoke_e2e.sh` 中长运行 `remote search` 被 caller `Ctrl-C` 后 Recent Calls 收敛到 `cancelled`，caller 不无限挂起。
- Human tests：`TC-RI-回归-113 ~ 117` 覆盖本地 relay、线上 relay、多命令取消、晚到结果不覆盖。
- 单元测试：`worker.rs` 的 `apply_cancelled_call_*` / `should_apply_call_result_*` / `settle_cancelled_call` 覆盖终态优先级。

## 产品语义

### 取消是终态操作，不是"暂停"

`Cancelled` 属于 terminal state 集合 `{Completed, Failed, Timeout, Cancelled}`。取消后不允许恢复到 `Streaming`，也不允许通过 `--resume-call-id` 复活。CLI 明确输出 `Remote command '<name>' cancelled by caller.` 并返回 exit code `130`（Unix 惯例 `128 + SIGINT`）。

### Recent Calls 状态颜色

Web UI `web/src/pages/Settings/tabs/RemoteInvokeTab.tsx` 中 `getCallStatusColor(call)` 统一映射：

- `cancelled` → 橙色 tag。
- `streaming` / `authorized` / `key_exchanged` / `pending` / `running` → processing（蓝色动画）。
- `completed` → `exit_code === 0` 绿色，非 0 红色。
- `failed` / `timeout` → 红色。
- 其他未知状态 → default。

## 技术细节

### Caller 侧取消协议

`crates/bifrost-cli/src/commands/remote.rs` 中：

- `wait_for_remote_call_cancel_signal()`（约 4700 行处）在 Unix 通过 `tokio::signal::unix::signal` 注册 `SIGINT` / `SIGTERM` / `SIGHUP`，并叠加 `tokio::signal::ctrl_c()` 兜底；非 Unix 仅监听 `ctrl_c()`。
- 正常路径 `open_call → subscribe_call_events`，与 `--resume-call-id` 跳过 `open_call` 的旁路，都用 `tokio::select!` 与 `wait_for_remote_call_cancel_signal()` 并行等待。
- 信号命中后：
  1. `caller.cancel_call(call_id, relay_token)` 立刻发出取消请求。
  2. 继续保持 `subscribe_call_events` SSE 订阅，等待 relay 返回终态 `Cancelled`。
  3. 若 SSE 阶段在短窗口内继续拿不到终态事件（例如 relay 429、连接中断），进入 `caller.settle_cancelled_call(...)` 分支：短超时重试拉取 `calls/:id/status` 或 `client/calls/:id` 得到最终结果；仍失败则合成 caller 侧 `Cancelled` 结果，避免 CLI 无限挂起。
- 该"合成 cancelled"兜底仅在 caller 已经成功发出 `POST /cancel` 之后生效，不影响普通成功路径。

### Client worker 侧取消执行

`crates/bifrost-admin/src/remote_invoke/worker.rs`：

- 每个 active call 挂一个 `ActiveCallControl`：
  ```rust
  struct ActiveCallControl {
      grant_id: String,
      started_at: u64,
      cancelled: AtomicBool,
      call_info: Mutex<Option<CallInfo>>,
      task: Mutex<Option<JoinHandle<()>>>,
      stdin_tx: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
  }
  ```
- `call_open` 启动远程执行任务后，把 `JoinHandle` 登记到 `ActiveCallControl.task`；shell / PTY 命令还会把 stdin channel 也挂到 `stdin_tx`，以便 cancel 时能同时中断输入流。
- 收到 `call_cancel` 时（统一入口 `apply_cancelled_call`）：
  1. 从 `active_calls` map 中 `remove(call_id)`；
  2. 命中：`mark_cancelled()` 置位 `AtomicBool`，`abort_task()` 中断执行任务，`mark_call_cancelled(duration_ms)` 把本地 `call_info` 推进到 `Cancelled` 并 `CallHistoryStore::upsert` 持久化；
  3. 未命中（句柄已被 GC）：从 `CallHistoryStore` 取出最近 `CallInfo`，按本地 `started_at` 计算 `duration_ms`，写回 `Cancelled`；
  4. 没有任何本地记录（call 从未在本 client 落地过）：仅 `debug!("cancel reconcile received before call history existed")`，不 panic、不重建。

### 终态优先级与竞态处理

所有本地状态转移由 `should_apply_call_result(current, next)` 集中守卫：

- `Cancelled` 是 terminal state；写入后 `Completed` / `Failed` / `Timeout` 不覆盖。
- `Completed` / `Failed` / `Timeout` 写入后，晚到 `Cancelled` 也不反向覆盖。
- `ActiveCallControl::update_call_result` 与仅供测试用的 `update_call_in_history` 均通过它判定。

SSE 重连或事件丢失场景，worker 通过定时器 `active_call_reconcile_ticker`（周期由常量 `ACTIVE_CALL_RECONCILE_INTERVAL_MS = 1000`ms 控制）触发 `reconcile_active_calls_with_relay()`：对仍登记为 active 的 call 反向查询 relay `client/calls/:id`，按 relay 终态补齐本地 `call_history`。

### Recent Calls UI

- Web UI Settings → Remote Invoke tab 的 Recent Calls 列表通过 `/api/remote-invoke/calls` 拉取，`cancelled` 显示为橙色。
- CLI `bifrost remote call list` / `bifrost remote call get <call_id>` 输出 `status=cancelled` 时，同样区分自定义 exit 与 caller cancel。

## CLI / Web / Admin API 表面

### CLI

- `bifrost remote search|status|traffic list|traffic get|file *|shell exec` 均支持 `Ctrl-C` 触发取消。
- 无独立 `bifrost remote cancel <id>` 子命令；取消入口即前台命令收到信号。远端仍可通过 admin API `/api/remote-invoke/calls/{call_id}` DELETE 或直接停 target 进程实现兜底。
- `bifrost remote call list --status cancelled` 支持过滤最近的取消调用。

### Web UI

- Settings → Remote Invoke → Recent Calls：
  - `cancelled` 橙色 tag；
  - 详情面板显示 `exit_code`（若被合成，caller 侧为 `null`；client 侧为进程实际 exit 或空）；
  - `ended_at - started_at` 计算 `duration_ms`。

### Admin API

- `POST /_bifrost/api/remote-invoke/calls/{call_id}/cancel`（内部使用）：等价于 relay 的 cancel。
- `GET /_bifrost/api/remote-invoke/calls?status=cancelled&limit=50`：分页拉取取消调用。
- `GET /_bifrost/api/remote-invoke/calls/{call_id}`：单条详情。
- Relay 侧：`POST /v4/remote-invoke/calls/:call_id/cancel`（现有）。

## Sync 边界

取消事件属于 target 本地状态机，不参与 Bifrost Sync：

- `Cancelled` 只写入本地 `CallHistoryStore` JSONL，不上行到 relay 之外的存储。
- 若未来跨设备"查看我在别的 client 上取消了什么"，需要独立 sync 通道设计，不复用当前 remote invoke 授权模型。

## Phase 1-4 实施路径

### Phase 1：Caller 信号识别与统一取消入口

- 抽出 `wait_for_remote_call_cancel_signal()` 到共享 helper。
- 所有 remote 命令 handler 用 `tokio::select!` 并行监听信号。
- 收到信号后立即 `cancel_call`，短窗口内继续 SSE 收敛。

### Phase 2：Client worker 真正 abort 与终态守卫

- 引入 `ActiveCallControl`，登记 `JoinHandle` 与 `stdin_tx`。
- 引入 `apply_cancelled_call` 统一入口。
- 引入 `should_apply_call_result` 集中判定终态优先级。
- `mark_call_cancelled(duration_ms)` 持久化到 `CallHistoryStore`。

### Phase 3：Reconcile 兜底与 Recent Calls UI

- `active_call_reconcile_ticker` 周期性对 active call 拉 relay 终态。
- `settle_cancelled_call` 处理 caller 侧兜底：合成 `Cancelled` 结果避免无限挂起。
- Web UI Recent Calls 统一状态颜色。

### Phase 4：文档、E2E、Human tests

- `human_tests/remote-invoke.md` 新增 / 更新 `TC-RI-回归-113 ~ 117`。
- `e2e-tests/tests/test_remote_invoke_e2e.sh` 添加 caller Ctrl-C → `cancelled` 断言。
- `human_tests/readme.md` 索引。

## 测试方案

### 单元测试（`crates/bifrost-admin/src/remote_invoke/worker.rs` 内 `mod tests`）

- `apply_cancelled_call_updates_active_call_and_persists_history`（worker.rs:7276）
- `apply_cancelled_call_updates_persisted_call_when_not_active`（worker.rs:7302）
- `apply_cancelled_call_noop_when_unknown_call`（worker.rs:7321）
- `should_apply_call_result_rejects_update_after_cancelled`（worker.rs:6676）
- `should_apply_call_result_rejects_cancel_after_completed`（worker.rs:6684）
- `ActiveCallControl` helpers（worker.rs:6537 ~ 6624）：cancelled flag、abort_task、stdin_tx 释放。
- `finalize_non_terminal_restored_calls_marks_streaming_failed`（call_history_store.rs:1046）：进程重启后遗留 `streaming` 记录被识别为异常。

### CLI 单元测试（`crates/bifrost-cli/src/commands/remote.rs`）

- `status=cancelled` 被识别为终态（返回 exit code 130）。
- `streaming` / `authorized` / `pending` 不误判为终态。
- `settle_cancelled_call` 在 relay 返回 429 时短重试，仍失败则合成 `Cancelled`。

### E2E 测试

- `e2e-tests/tests/test_remote_invoke_e2e.sh`：
  - 触发长运行 `remote search`；
  - 向 caller 进程发送 `SIGINT`；
  - 轮询 `/_bifrost/api/remote-invoke/calls` 直到该 `call_id` `status=cancelled`；
  - 断言 caller 进程退出码 `130`；
  - 连续 3 次轮询稳定保持 `cancelled`，验证不被晚到结果覆盖。

### Human tests（`human_tests/remote-invoke.md`）

- `TC-RI-回归-113`：`remote search` 被 caller 主动取消后 Recent Calls 显示 `cancelled`（本地 relay）。
- `TC-RI-回归-114`：`remote status` / `remote traffic list` / `remote traffic get` / `remote search` 全部支持 caller 取消。
- `TC-RI-回归-115`：取消后的晚到结果不覆盖 `cancelled`（连续 3 次轮询保持）。
- `TC-RI-回归-116`：本地 relay 大流量 `search.get` 取消不会因 relay 429 让 caller 无限挂起。
- `TC-RI-回归-117`：线上 relay 下 caller `Ctrl-C` 后 target client 进入 `cancelled` 且 caller 不无限挂起（真实样本 `call_id=412ef871902f0195`，`exit_code=130`）。

所有 human tests 启动服务必须使用临时 `BIFROST_DATA_DIR`、非 9900 admin 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

## Review / Fix / Test 闭环

### 第 1 轮

- 复核用户目标：所有 remote 命令都支持 caller 取消？三侧收敛 `cancelled`？晚到结果不覆盖？
- 复核 diff：`worker.rs` 是否覆盖非 shell 命令（file.*、query.readonly）；CLI 是否对每个 handler 都接入 `wait_for_remote_call_cancel_signal`？
- 重点 review：
  - `apply_cancelled_call` 是否在 active_calls remove 之前抓够 `CallInfo` 快照；
  - `should_apply_call_result` 是否被所有写入路径调用；
  - reconcile 循环是否会把已经 terminal 的 call 重新拉回 active。
- 复测：worker unit tests、CLI unit tests、`test_remote_invoke_e2e.sh` caller Ctrl-C 场景。

### 第 2 轮

- 复核第 1 轮问题的修复。
- 再次检查 `git status --short` / `git diff`，确保 human_tests 索引同步更新。
- 重点 review：
  - `settle_cancelled_call` 合成 `Cancelled` 时不能污染 relay 侧真实终态；
  - Web UI 颜色是否覆盖所有已知 status；
  - 线上 relay 场景是否有额外的 `429` 抖动窗口需要调 caller 短重试上限。
- 复测：Human tests `TC-RI-回归-113 ~ 117` 全部复跑。

## 风险与决策

- **合成 `Cancelled` 与真实 relay 终态冲突**：caller 兜底合成的 `Cancelled` 只写入 caller 本地 CLI 输出，不回写 target client 的 `CallHistoryStore`；target 端最终终态仍以 `apply_cancelled_call` / reconcile 为准。
- **`SIGHUP` 语义**：某些 shell wrapper 会在断开时对子进程发送 `SIGHUP`，caller 侧统一识别为"用户主动取消"。若未来出现"我只想脱离 tty 但保留 remote 调用"的需求，需要单独引入 `--detach` 语义，不能改变现有信号处理。
- **`--resume-call-id` 与取消**：resume 场景下允许对已发起的 call 主动 cancel；关键是 relay/target 已知 call_id 存在。
- **reconcile 频率**：`ACTIVE_CALL_RECONCILE_INTERVAL_MS = 1000` 已在实测下平衡了收敛速度与 relay 压力；若未来接入更多 client，可以退化为指数退避。
- **未来扩展**：`bifrost remote call cancel <id>` 独立子命令未来可从 admin API 派生，不在本方案范围。
