# Cooperative Long Task Loop 真实场景测试

## 功能模块说明

验证 Agent 长任务挂起方案：`exec_command` 启动安装、CI、构建、dev server、delegated agent 等长任务后，Agent turn 可以进入 `waiting_on_session`，由 runtime watcher 低成本等待新输出、退出、超时或用户新消息，再用压缩事件摘要恢复模型 loop，避免模型为了空轮询反复进入完整 Agent turn。

本文件覆盖 Agent 长任务挂起的设计验收、当前 Phase 1/1.5 实现验收，以及 Cooperative Long Task Loop（协作式长任务循环）的方案静态验收。后续如果继续推进跨请求后台恢复，需要补齐真实 `/agent/chat` waiting 响应和 IM 卡片长时间挂起恢复链路。

## 前置条件

1. 当前目录位于仓库根目录。
2. 本轮 Phase 1 实现验证以 Rust 单元测试和 bifrost-e2e runner 为主，不启动长期驻留的 Bifrost 服务，不修改系统代理。
3. 执行下列 shell 命令时，按项目规则在命令前加 `source ~/.zshrc &&` 初始化 shell。
4. 后续跨请求 `/agent/chat` 或 IM/WebUI 实现阶段启动 Bifrost 时必须使用临时 `BIFROST_DATA_DIR`、非正式端口，并携带 `--no-system-proxy`。

## 测试用例列表

### TC-ALT-01：长任务挂起设计协议静态验收

**操作步骤**：

1. 确认设计文档存在：
   ```bash
   test -f design/agent-long-task-suspension.md
   ```
2. 确认设计文档覆盖关键协议：
   ```bash
   rg -n "waiting_on_session|RuntimeExecWatcher|LongTaskProfile|ExecOutputCursor|unchanged=true|TurnSuspended|LongTaskStatus|/stop|WatcherLost|Phase 1" design/agent-long-task-suspension.md
   ```
3. 确认设计文档包含 Codex 源码级对照：
   ```bash
   rg -n "execute_to_pending|wait_to_pending|PendingRuntimeMode::PauseUntilResumed|start_streaming_output|spawn_exit_watcher|collect_output_until_deadline|command/exec/outputDelta|backoff" design/agent-long-task-suspension.md
   ```
4. 确认设计文档声明不改变既有工具契约：
   ```bash
   rg -n '不改变 `exec_command` 和 `write_stdin`|短任务即时返回|交互任务' design/agent-long-task-suspension.md
   ```
5. 确认 human_tests 索引包含本文件：
   ```bash
   rg -n "agent-long-task-suspension.md" human_tests/readme.md
   ```
6. 确认设计文档包含 Codex 可媲美性门禁：
   ```bash
   rg -n "Codex 可媲美性标准|resume_seq|at-most-once|output_drained|ProcessPruned|model_request_count_while_waiting|单一事实源|资源治理门禁" design/agent-long-task-suspension.md
   ```

**预期结果**：

- 所有命令退出码为 0。
- 设计文档包含挂起态、runtime watcher、动态退避、输出游标、摘要去重、IM/WebUI 进度、取消、持久化和终极形态交付边界。
- 设计文档包含 `~/work/github/codex` 中 code-mode、unified_exec、app-server command/exec 和 retry/backoff 的源码级对照。
- 设计文档包含状态机、响应通道、输出事实源、资源治理、可观测指标和 at-most-once resume 等 Codex 可媲美性门禁。
- 索引能定位到 `agent-long-task-suspension.md`。

**实际结果**：

- 通过。2026-05-24 按项目规则带 `source ~/.zshrc &&` 执行上述 6 条命令均成功；第 4 条命令使用单引号保护反引号，避免 zsh 命令替换。设计文档包含 `waiting_on_session`、`RuntimeExecWatcher`、`LongTaskProfile`、`ExecOutputCursor`、`unchanged=true`、`TurnSuspended`、`LongTaskStatus`、`/stop`、`WatcherLost` 和 `Phase 1`，并纳入 `execute_to_pending`、`wait_to_pending`、`PendingRuntimeMode::PauseUntilResumed`、`start_streaming_output`、`spawn_exit_watcher`、`collect_output_until_deadline`、`command/exec/outputDelta` 与 `backoff` 等 Codex 源码对照，同时明确不改变既有 `exec_command` / `write_stdin` 契约。设计文档还包含 `resume_seq`、at-most-once resume、`output_drained`、`ProcessPruned`、`model_request_count_while_waiting`、单一事实源和资源治理门禁。

---

### TC-ALT-02：真实 `/agent/chat` 长任务首次返回 waiting 且不重复模型轮询

**操作步骤**：

1. 启动 mock model server，使第一轮响应调用：
   ```text
   exec_command(cmd="printf start; sleep 6; printf done", yield_time_ms=250)
   ```
2. 使用临时数据目录启动真实 Bifrost：
   ```bash
   TEST_DIR="$(mktemp -d)"
   BIFROST_DATA_DIR="$TEST_DIR" cargo run --bin bifrost -- start \
     --host 127.0.0.1 \
     -p 18898 \
     --unsafe-ssl \
     --no-system-proxy
   ```
3. 调用真实 `/agent/chat`。
4. 2 秒后检查 mock model server 请求计数。

**预期结果**：

- 首次响应包含 `status = waiting_on_session`、`session_id`、strategy/evidence 和初始输出摘要。
- 2 秒内 mock model server 只有 1 次请求，不会为了空轮询重复请求模型。
- IM/WebUI 进度状态显示长任务仍在运行。

**实际结果**：

- 待实现后执行。

---

### TC-ALT-03：长任务退出后恢复模型并保留 exit code 和新增输出

**操作步骤**：

1. 复用 TC-ALT-02 的环境。
2. 等待命令输出 `done` 并退出。
3. 观察 watcher 恢复后的最终 `/agent/chat` 响应或事件流。
4. 检查 session history / mock model 输入摘要。

**预期结果**：

- watcher 因 `ProcessExited` 唤醒模型。
- 恢复摘要包含 `session_id`、strategy/evidence、elapsed、`exit_code = 0`、新增输出 `done`。
- history 中没有多条 unchanged heartbeat tool result。
- mock model server 总请求次数为 2 左右：一次启动命令，一次处理退出摘要。

**实际结果**：

- 待实现后执行。

---

### TC-ALT-04：用户 `/stop` 能终止等待中的长任务并清理 watcher

**操作步骤**：

1. 启动会进入 waiting 的长任务：
   ```text
   exec_command(cmd="sleep 60", yield_time_ms=250)
   ```
2. 等待状态变为 `waiting_on_session`。
3. 发送 `/stop` 或调用对应 cancel API。
4. 检查进程、watcher 和 progress card 状态。

**预期结果**：

- 等待中的 child process 被终止。
- watcher task 清理。
- `WaitingOnExecSession` 被移除。
- IM/WebUI 状态变为 stopped，不再继续恢复模型执行。

**实际结果**：

- 待实现后执行。

---

### TC-ALT-05：IM progress card 等待期间只刷新状态，不生成自然语言刷屏

**操作步骤**：

1. 配置 IM provider 后，通过 IM 消息触发一个会进入 waiting 的长任务。
2. 观察同一张 progress card 的更新。
3. 在任务无新输出期间等待至少 2 个 watcher heartbeat。
4. 等任务退出后观察最终卡片。

**预期结果**：

- 等待期间仍是同一张 progress card。
- 无新输出 heartbeat 不生成新的自然语言消息。
- 工具执行状态区显示 strategy/evidence、运行时长、最后输出摘要和下次检查时间。
- 任务退出后 final flush 包含最终输出和 exit code，并关闭 streaming。

**实际结果**：

- 待实现后执行。

---

### TC-ALT-06：Codex 可媲美性门禁回归

**操作步骤**：

1. 人为注入两个相同 `resume_seq` 的 watcher exit event。
2. 模拟进程退出后仍有 trailing output 到达。
3. 模拟模型恢复请求失败。
4. 将 live exec session 数推到上限，触发资源治理清理。
5. 检查 history、progress card、metrics 和最终状态。

**预期结果**：

- 重复 `resume_seq` 只触发一次模型请求。
- final summary 等待 `output_drained` 或 grace window，包含 trailing output。
- 恢复失败不会丢失 `WaitingOnExecSession`，而是记录 `ResumeFailed` 并等待下一次事件。
- 资源治理清理运行中任务时生成 `ProcessPruned`，UI/模型都能看到明确终态。
- 指标包含 `model_request_count_while_waiting`、`long_task_heartbeat_omitted_from_history_count` 和 `long_task_resume_count{reason=...}`。

**实际结果**：

- 待实现后执行。

---

### TC-ALT-07：Phase 1 runtime watcher 实现验收

**操作步骤**：

1. 执行 agent turn loop 长任务单元验证：
   ```bash
   cargo test -p bifrost-agent exec_command_long_task_waits_in_runtime_without_model_polling -- --nocapture
   ```
2. 执行 watcher jitter 单元验证：
   ```bash
   cargo test -p bifrost-agent long_task_watch_interval_includes_bounded_jitter -- --nocapture
   ```
3. 执行 exec runtime poll 增量/unchanged 验证：
   ```bash
   cargo test -p bifrost-agent runtime_poll_exec_session_reports_unchanged_without_model_tool_call -- --nocapture
   ```
4. 执行 bifrost-e2e runner 长任务 runtime watcher 验证：
   ```bash
   cargo run -p bifrost-e2e -- --test im_gateway_agent_long_task_runtime_watch --test-timeout 30
   ```

**预期结果**：

- `exec_command` 首次返回 running session 后，turn loop 进入 runtime watcher 等待，不让模型调用 `write_stdin` 轮询。
- 普通未归类后台命令继续保留模型可见 `session_id`，既有 `write_stdin` 轮询兼容路径不被 runtime watcher 抢占。
- 长任务进程退出后，模型只收到一条压缩 tool result，包含 `resume_reason=process_exited`、最终输出和 `model_request_count_while_waiting=0`。
- 该用例显式关闭长期记忆读写，mock model server 总请求数为 2：第一次请求产生 `exec_command`，第二次请求处理最终 tool result。
- runtime poll 能返回 `unchanged=true` 和 `new_output_bytes=0`，不重复旧输出。
- turn events 包含 `TurnSuspended`、`LongTaskExited` 和 `TurnResumed`。

**实际结果**：

- 通过。2026-05-24 按项目规则带 `source ~/.zshrc &&` 执行上述 4 条命令均成功；`im_gateway_agent_long_task_runtime_watch` 通过，断言模型请求数为 2，工具输出包含 `process_exited`、`start`、`done` 和 `model_request_count_while_waiting`。
- 通过。2026-05-29 CI 回归发现 worker 确定性执行 memory extraction 后，未隔离的 mock 请求计数会包含 `Memory Writing Agent: Phase 1`，导致本用例从 2 变 3。已将该用例显式设置 `memories.generate_memories=false`、`memories.use_memories=false`，保持测试目标聚焦在长任务 watcher 是否避免模型轮询。

---

### TC-ALT-08：Phase 1.5 runtime notify 等待修复验收

**操作步骤**：

1. 检查设计文档已明确 runtime notify 等待边界：
   ```bash
   rg -n "Phase 1.5|Notify|stdout/stderr reader|exit watcher|运行中旧服务" design/agent-long-task-suspension.md
   ```
2. 检查 `ExecSession` 等待路径由输出/退出通知唤醒：
   ```bash
   rg -n "notify: Notify|notify_waiters|timeout\\(remaining, self.notify.notified\\(\\)|runtime_poll_exec_session_wakes_on_output_before_deadline|adaptive_long_task_metadata_is_runtime_state_based" crates/agent/src/tools/exec_command.rs
   ```
3. 检查 adaptive 等待窗口不会在 300 秒后把 session 交还给模型轮询，且 runtime poll/probe 最大退避为 300 秒：
   ```bash
   rg -n "ADAPTIVE_LONG_TASK_PROFILE|ADAPTIVE_LONG_TASK_MAX_WAIT_MS|3_900_000|ADAPTIVE_LONG_TASK_MAX_INTERVAL_MS|300_000|adaptive_long_task_watch_extends_beyond_background_terminal_timeout" crates/agent/src/session/turn_loop.rs crates/agent/src/session/tests.rs
   ```
4. 执行 exec runtime poll 增量/unchanged 验证：
   ```bash
   cargo test -p bifrost-agent runtime_poll_exec_session_reports_unchanged_without_model_tool_call -- --nocapture
   ```
5. 执行 runtime notify 提前唤醒验证：
   ```bash
   cargo test -p bifrost-agent runtime_poll_exec_session_wakes_on_output_before_deadline -- --nocapture
   ```
6. 执行自适应 metadata 运行事实验证：
   ```bash
   cargo test -p bifrost-agent adaptive_long_task_metadata_is_runtime_state_based -- --nocapture
   ```
7. 执行 adaptive 等待窗口验证：
   ```bash
   cargo test -p bifrost-agent adaptive_long_task_watch_extends_beyond_background_terminal_timeout -- --nocapture
   ```
8. 执行长任务模型请求数验证：
   ```bash
   cargo test -p bifrost-agent exec_command_long_task_waits_in_runtime_without_model_polling -- --nocapture
   ```

**预期结果**：

- 文档明确 Phase 1.5 不改变 `exec_command` / `write_stdin` 协议，只把 runtime 等待从固定 tick 改为通知唤醒。
- `ExecSession` 具备 session-local `Notify`，stdout/stderr reader、reader EOF/错误和 exit watcher 会唤醒等待中的 runtime poll。
- 无新增输出时仍返回 `unchanged=true` 和 `new_output_bytes=0`。
- 长 deadline 下输出到达后 poll 必须提前返回，证明不是等 deadline/loop tick 后才检查。
- 命令关键字、正则、GitHub Actions 脚本路径和工程专属路径不得参与长任务分类；任意非 TTY 命令只要 initial yield 后仍在运行，就基于运行事实进入 `adaptive` 候选。
- adaptive 等待窗口覆盖一小时级命令；不会因为 background terminal 默认 300 秒上限触发 `resume_reason=timeout` 并把仍在运行的 `session_id` 暴露给模型继续轮询。
- 明确运行中旧服务必须重启到最新二进制后才能生效，不能把旧进程行为误判为当前代码失败。
- 长任务等待期间模型请求数不因空轮询增长。

**实际结果**：

- 通过。2026-05-24 按项目规则带 `source ~/.zshrc &&` 执行原 notify 静态检查与单元测试均成功。2026-05-25 针对通用 coding agent 约束修正：移除命令关键字/正则分类器，改为基于 initial yield 后仍在运行的事实标记 `adaptive`；已执行本用例更新后的静态检查，且 `cargo test -p bifrost-agent adaptive_long_task_metadata_is_runtime_state_based -- --nocapture`、`cargo test -p bifrost-agent adaptive_long_task_watch_extends_beyond_background_terminal_timeout -- --nocapture`、`cargo test -p bifrost-agent exec_command_long_task_waits_in_runtime_without_model_polling -- --nocapture` 均通过。

### TC-ALT-09：Cooperative Long Task Loop 方案静态验收

**操作步骤**：

1. 确认方案文档已升级命名，并保留旧文件名兼容说明：

   ```bash
   rg -n "Cooperative Long Task Loop|协作式长任务循环|Agent Long Task Suspension" design/agent-long-task-suspension.md
   ```

2. 确认方案明确拆分执行事实源、自动监控器和模型控制权：

   ```bash
   rg -n "执行事实源|自动监控器|模型控制权|ExecSession|RuntimeExecMonitor|model-visible monitor contract" design/agent-long-task-suspension.md
   ```

3. 确认方案包含 append-only transcript、双 cursor 和非破坏性读取要求：

   ```bash
   rg -n "append-only transcript|ExecOutputChunk|watcher_cursor|model_visible_cursor|manual poll|非破坏性读取" design/agent-long-task-suspension.md
   ```

4. 确认方案包含 monitor registry、resume queue、`resume_seq` 去重和 turn loop 不阻塞等待：

   ```bash
   rg -n "WatcherRegistry|AgentResumeQueue|resume_seq|WaitingOnExecSession|run_turn|不在 turn 内阻塞|enqueue resume" design/agent-long-task-suspension.md
   ```

5. 确认模型可控协议包含 `exec_command.watch`、`write_stdin` 手动 poll 和 `exec_monitor` 策略控制：

   ```bash
   rg -n "exec_command\\.watch|write_stdin|exec_monitor|set_policy|manual_only|resume_auto|stop" design/agent-long-task-suspension.md
   ```

6. 确认 history 策略和 UI 语义明确不把 heartbeat 写入 history，也不因 UI-only 更新触发模型请求：

   ```bash
   rg -n "heartbeat-only|unchanged poll|UI-only|不发 model request|不写 conversation history|model_request_count_while_waiting" design/agent-long-task-suspension.md
   ```

7. 确认 human_tests 索引包含本文件和 Cooperative Long Task Loop 条目：

   ```bash
   rg -n "agent-long-task-suspension.md.*Cooperative Long Task Loop" human_tests/readme.md
   ```

**预期结果**：

- 所有 `rg` 命令退出码为 0。
- 设计文档明确当前 Phase 1/1.5 仍未完成跨 HTTP 请求后台挂起恢复、append-only transcript、完整 cursor、`resume_seq` 去重和 pending frontier。
- 设计文档明确目标机制是 runtime monitor registry + resume queue，而不是 turn loop 自己长时间阻塞等待。
- 设计文档明确模型显式知情，且能 manual poll、写 stdin、stop、`set_policy`、`manual_only`、`resume_auto`。

**实际结果**：

- 通过。2026-05-25 按项目规则带 `source ~/.zshrc &&` 执行上述 7 条 `rg` 静态验收命令均成功。设计文档已升级为 Cooperative Long Task Loop，明确三权分离、append-only transcript、`RuntimeExecMonitor`、`WatcherRegistry`、`AgentResumeQueue`、`resume_seq`、`watcher_cursor` / `model_visible_cursor`、`exec_command.watch`、`write_stdin` manual poll、`exec_monitor` 的 `set_policy` / `manual_only` / `resume_auto` / `stop`，并明确 heartbeat-only / unchanged poll / UI-only 更新不写 conversation history、不触发 model request。`human_tests/readme.md` 已同步本文件 9 个用例计数。

### TC-ALT-10：终极形态时长场景静态验收

**操作步骤**：

1. 确认方案明确不再以分阶段实现作为目标形态，而以终极形态为验收边界：

   ```bash
   rg -n "终极形态|最终交付标准|不是.*分阶段|实现可以按切片推进" design/agent-long-task-suspension.md
   ```

2. 确认方案覆盖真实时长 case rundown：

   ```bash
   rg -n "真实时长 Case Rundown|即时完成|短任务|分钟级|半小时级|数小时级" design/agent-long-task-suspension.md
   ```

3. 确认方案包含短异步任务的 short completion grace 收敛策略：

   ```bash
   rg -n "short_completion_grace|short_grace_then_suspend|short_grace_ms|数秒级短异步任务" design/agent-long-task-suspension.md
   ```

4. 确认方案覆盖半小时级构建任务的 token 节省策略：

   ```bash
   rg -n "半小时级|编译 Bifrost|cargo test --workspace --all-features|model_request_count_while_waiting=0|普通编译进度只进 UI" design/agent-long-task-suspension.md
   ```

5. 确认方案覆盖数小时级任务的持久化、外部状态源和重启可解释恢复：

   ```bash
   rg -n "数小时级|持久化 waiting state|external_status_source|GitHub API|WatcherLost|ProcessDetached" design/agent-long-task-suspension.md
   ```

6. 确认方案给模型暴露 duration class 与 response strategy，避免模型误判需要轮询：

   ```bash
   rg -n "duration_class|response_strategy|instant\\|short\\|medium\\|long\\|very_long|inline_until_yield|return_on_suspend|detached_monitor" design/agent-long-task-suspension.md
   ```

7. 确认 human_tests 索引包含终极形态时长场景说明：

   ```bash
   rg -n "agent-long-task-suspension.md.*终极形态" human_tests/readme.md
   ```

**预期结果**：

- 所有 `rg` 命令退出码为 0。
- 设计文档明确秒级任务不应因长任务机制引入额外 token 或等待噪音。
- 设计文档明确数秒级短异步任务可用 short grace 在不增加模型请求的情况下收敛。
- 设计文档明确分钟级、半小时级和数小时级任务都应由 runtime monitor 消化等待，heartbeat/UI-only 更新不触发模型请求。
- 设计文档明确数小时级任务必须有持久化 waiting state、外部状态源恢复和 `WatcherLost` / `ProcessDetached` 可解释终态。

**实际结果**：

- 通过。2026-05-25 按项目规则带 `source ~/.zshrc &&` 执行上述 7 条 `rg` 静态验收命令均成功。设计文档已明确终极形态交付边界，不再把分阶段实现作为目标方案；同时补齐即时完成、数秒级短异步任务、分钟级任务、半小时级构建/全量验证和数小时级任务的 runtime 行为、HTTP/IM 行为、模型请求策略、short completion grace、duration class、response strategy、持久化 waiting state、外部状态源和 `WatcherLost` / `ProcessDetached` 可解释恢复。`human_tests/readme.md` 已同步本文件 10 个用例计数。

### TC-ALT-11：安全与抗幻觉静态验收

**操作步骤**：

1. 确认方案明确 runtime 是事实权威，模型只能请求控制动作：

   ```bash
   rg -n "runtime 仍是唯一执行者和仲裁者|模型不能直接拥有 monitor|模型只发出控制请求|runtime 基于 owner" design/agent-long-task-suspension.md
   ```

2. 确认 `exec_monitor` 控制面必须做 owner/state/strategy 校验、policy clamp 和 accepted/rejected/degraded 返回：

   ```bash
   rg -n "monitor_id.*必须存在|exec_session_id.*绑定|set_policy.*clamp|accepted / rejected / degraded|rejected_reason|effective_policy" design/agent-long-task-suspension.md
   ```

3. 确认 `RuntimeMonitorEvent` 有结构化事实字段和 allowed next actions：

   ```bash
   rg -n "RuntimeMonitorContext|terminal_state|output_lossy|lost_chunk_count|model_request_count_while_waiting|allowed_next_actions|ExternalTaskStatus" design/agent-long-task-suspension.md
   ```

4. 确认模型不能基于非 terminal event 宣称任务完成：

   ```bash
   rg -n "非 terminal event 不允许|terminal_state=exited|external_status\\.terminal=true|resume_reason=output_available.*不能说明成功或失败" design/agent-long-task-suspension.md
   ```

5. 确认 lossy output、timeout、WatcherLost、ProcessDetached 都有可解释约束：

   ```bash
   rg -n "output_lossy=true|lost_chunk_count>0|timeout 不是失败事实|WatcherLost.*ProcessDetached|不得声称任务成功完成" design/agent-long-task-suspension.md
   ```

6. 确认 prompt/tool schema 有抗幻觉规则：

   ```bash
   rg -n 'Do not claim a command is complete|Only request actions listed in allowed_next_actions|exec_monitor actions are requests|accepted/rejected/degraded|不让模型从 `summary` 文本反推 terminal state' design/agent-long-task-suspension.md
   ```

7. 确认测试方案包含对应回归测试：

   ```bash
   rg -n "exec_monitor_set_policy_out_of_range_is_rejected_or_degraded|manual_only_keeps_exit_watcher_and_ui_heartbeat|stop_cannot_cross_monitor_or_session_boundary|non_terminal_output_event_cannot_be_treated_as_success|lossy_output_resume_requires_incomplete_output_notice|allowed_next_actions_limits_model_control_requests" design/agent-long-task-suspension.md
   ```

8. 确认 human_tests 索引包含安全控制面和抗幻觉说明：

   ```bash
   rg -n "agent-long-task-suspension.md.*抗幻觉" human_tests/readme.md
   ```

**预期结果**：

- 所有 `rg` 命令退出码为 0。
- 设计文档明确模型动作只是请求，runtime 必须校验、拒绝或降级。
- 设计文档明确非终态输出不能被模型总结为成功完成。
- 设计文档明确 lossy output、timeout、WatcherLost、ProcessDetached 都必须如实披露。
- 设计文档明确工具 schema 和 prompt 规则会限制模型误判、越权控制和基于 summary 文本编造终态。

**实际结果**：

- 通过。2026-05-25 按项目规则带 `source ~/.zshrc &&` 执行上述 8 条 `rg` 静态验收命令均成功。设计文档已明确 runtime 是事实权威，模型动作只是请求；`exec_monitor` 必须校验 monitor/session 绑定、state、strategy/evidence 和 policy clamp，并返回 accepted/rejected/degraded；`RuntimeMonitorEvent` 包含 terminal state、lossy output、cursor、external status 和 `allowed_next_actions` 等结构化事实字段；非 terminal output 不能被模型总结为成功完成；lossy output、timeout、WatcherLost、ProcessDetached 必须如实披露；prompt 和工具 schema 均要求模型只能请求允许动作，不能从 summary 文本反推终态。`human_tests/readme.md` 已同步本文件 11 个用例计数。

### TC-ALT-12：watchdog 与卡死兜底静态验收

**操作步骤**：

1. 确认方案新增 watchdog 与卡死兜底章节：

   ```bash
   rg -n "Watchdog 与卡死兜底|watchdog probe|完全没有数据|无进展证据" design/agent-long-task-suspension.md
   ```

2. 确认方案区分 `SilentRunning`、`StallSuspected` 和 `MonitorDegraded`：

   ```bash
   rg -n "SilentRunning|StallSuspected|MonitorDegraded|三类无数据状态" design/agent-long-task-suspension.md
   ```

3. 确认 runtime 有 30 秒级低成本 probe，但不会每次都唤醒模型：

   ```bash
   rg -n "30 秒级|probe every 30s|UI only|不等于模型请求|SilentRunning.*不恢复模型" design/agent-long-task-suspension.md
   ```

4. 确认 no-progress threshold、escalation 和 cooldown 防止重新变成模型轮询：

   ```bash
   rg -n "no-progress threshold|no-progress 升级策略|cooldown|同一 escalation level|model_acknowledged_stall_seq|long_task_stall_notice_suppressed_count" design/agent-long-task-suspension.md
   ```

5. 确认 probe 事实源覆盖进程、输出、资源、artifact、外部状态和 monitor 自身健康：

   ```bash
   rg -n "try_wait|OS process alive|CPU time|artifact mtime|GitHub Actions|monitor task health|notify receiver" design/agent-long-task-suspension.md
   ```

6. 确认 `RuntimeMonitorEvent` 包含 stall 相关结构化字段：

   ```bash
   rg -n "stall_state|last_output_age_ms|consecutive_silent_probes|no_progress_escalation_level|resume_reason=stall_suspected" design/agent-long-task-suspension.md
   ```

7. 确认测试方案包含 watchdog 回归测试：

   ```bash
   rg -n "silent_running_probe_updates_ui_without_model_request|stall_suspected_resumes_model_once_per_escalation_level|monitor_degraded_resumes_model_immediately|stall_acknowledgement_suppresses_immediate_repeat_notice|stall_suspected_cannot_be_treated_as_failure_or_success" design/agent-long-task-suspension.md
   ```

8. 确认 human_tests 索引包含 watchdog 和卡死兜底说明：

   ```bash
   rg -n "agent-long-task-suspension.md.*watchdog.*卡死" human_tests/readme.md
   ```

**预期结果**：

- 所有 `rg` 命令退出码为 0。
- 设计文档明确 runtime 会周期性做低成本 probe，不会完全沉默等待 10 或 20 分钟。
- 设计文档明确 SilentRunning 不触发模型请求，StallSuspected 会在 no-progress threshold 后低频通知模型，MonitorDegraded 会立即通知模型。
- 设计文档明确 cooldown、escalation level 和 acknowledgement 防止 watchdog 变成新的 token 轮询。
- 设计文档明确 stall_suspected 不是 terminal event，模型不能据此宣称成功或失败。

**实际结果**：

- 通过。2026-05-25 按项目规则带 `source ~/.zshrc &&` 执行上述 8 条 `rg` 静态验收命令均成功。设计文档已补充 watchdog 与卡死兜底机制，明确 runtime 会做 30 秒级低成本 probe；`SilentRunning` 只更新 UI 和 runtime metadata，不写 history、不触发模型；`StallSuspected` 在 no-progress threshold 后按 escalation/cooldown 低频恢复模型；`MonitorDegraded` 在 probe/notify/child handle 异常时立即恢复模型；probe 覆盖 OS process alive、transcript cursor、CPU/resource、artifact mtime、GitHub Actions 等外部状态和 monitor task health；`RuntimeMonitorEvent` 增加 stall 结构化字段，且 `stall_suspected` 不是 terminal event。`human_tests/readme.md` 已同步本文件 12 个用例计数。

### TC-ALT-13：scheduled polling 与 event monitor 协同静态验收

**操作步骤**：

1. 确认方案新增 scheduled polling 与 event monitor 协同章节：

   ```bash
   rg -n "Scheduled Polling 与 Event Monitor 协同|scheduled polling|event monitor|runtime 内部的 scheduled polling" design/agent-long-task-suspension.md
   ```

2. 确认未知时长命令有 discovery polling 和 N 次退避策略：

   ```bash
   rg -n '未知时长 discovery polling|DiscoveryPolling|poll #1: 1s|poll #5: 15s|默认建议 `N=5`|Discovery budget' design/agent-long-task-suspension.md
   ```

3. 确认达到 N 次 unchanged/alive poll 后提升 monitor ownership，而不是让模型继续轮询：

   ```bash
   rg -n "达到 N 次|promote to long monitor ownership|提升 monitor ownership|poll_count=N|不能让模型继续空 poll" design/agent-long-task-suspension.md
   ```

4. 确认长任务保留 poll deadline 作为 checkpoint，monitor semantic event 可以抢占：

   ```bash
   rg -n "长任务中的 poll deadline|poll deadline|monitor semantic event|preempt poll deadline|cancel/reset 当前 poll timer" design/agent-long-task-suspension.md
   ```

5. 确认 `ScheduledPollState` 和 `PollPhase` 被纳入 monitor 状态：

   ```bash
   rg -n "ScheduledPollState|PollPhase|Discovery|Monitored|poll_state|promoted_to_monitor_at_ms" design/agent-long-task-suspension.md
   ```

6. 确认 metrics 和测试覆盖 discovery poll、promotion、poll checkpoint、monitor 抢占和 `resume_seq` 去重：

   ```bash
   rg -n "long_task_discovery_poll_count|long_task_discovery_promoted_count|long_task_poll_deadline_checkpoint_count|long_task_monitor_preempted_poll_count|discovery_poll_promotes_after_n_unchanged_alive_polls|monitor_event_preempts_scheduled_poll_deadline|simultaneous_poll_and_monitor_event_dedupes_by_resume_seq" design/agent-long-task-suspension.md
   ```

7. 确认 human_tests 索引包含 scheduled polling 与 monitor 协同说明：

   ```bash
   rg -n "agent-long-task-suspension.md.*scheduled polling.*monitor" human_tests/readme.md
   ```

**预期结果**：

- 所有 `rg` 命令退出码为 0。
- 设计文档明确一开始未知时长时先由 runtime discovery polling 退避判断，不让模型反复 poll。
- 设计文档明确 N 次退避后进入 monitor ownership，monitor 优先处理恢复模型。
- 设计文档明确长任务中仍有 poll deadline，但 poll deadline 默认只做 checkpoint，monitor notify 可抢占。
- 设计文档明确 poll 与 monitor 同时到达时用 priority 和 `resume_seq` 去重。

**实际结果**：

- 通过。2026-05-25 按项目规则带 `source ~/.zshrc &&` 执行上述 7 条 `rg` 静态验收命令均成功。设计文档已补充 scheduled polling 与 event monitor 协同机制：未知时长命令先由 runtime discovery polling 按 `1s/2s/4s/8s/15s` 退避探测，默认 `N=5`；达到 N 次 unchanged/alive 或 discovery budget 后提升 monitor ownership，避免模型继续空 poll；长任务仍保留 poll deadline 作为 liveness/external-status checkpoint；monitor semantic event 可在 poll deadline 前抢占并 cancel/reset poll timer；同时到达时通过 priority 和 `resume_seq` 去重。`human_tests/readme.md` 已同步本文件 13 个用例计数。

### TC-ALT-14：合并状态机与 5 分钟退避上限静态验收

**操作步骤**：

1. 确认方案使用合并状态机，顶层状态收敛：

   ```bash
   rg -n "Cooperative Long Task Loop 合并状态机|ActiveTurn|MonitoringExec|ResumingTurn|Terminal" design/agent-long-task-suspension.md
   ```

2. 确认 discovery / monitored / stall 等细节只是 `MonitoringExec` 内部 substate，不再膨胀顶层状态：

   ```bash
   rg -n "MonitoringExec.*substate|DiscoveryPolling|Monitored|SilentRunning|StallSuspected|MonitorDegraded|不是顶层 turn state" design/agent-long-task-suspension.md
   ```

3. 确认 terminal 状态通过 `terminal_state` 字段区分：

   ```bash
   rg -n 'Completed.*Stopped.*Timeout.*WatcherLost.*ProcessPruned|统一归入 `Terminal`|terminal_state 字段区分' design/agent-long-task-suspension.md
   ```

4. 确认模型通知退避上限为 5 分钟 / 300 秒：

   ```bash
   rg -n "模型通知退避硬上限为 5 分钟|不得退避超过 5 分钟|退避最大值也是 300s|300s 退避硬上限" design/agent-long-task-suspension.md
   ```

5. 确认 runtime poll/probe 最大间隔不超过 300 秒：

   ```bash
   rg -n "30s -> 60s -> 120s -> 300s 上限|runtime poll/probe 最大间隔可以到 300s|统一退避上限" design/agent-long-task-suspension.md
   ```

6. 确认 terminal、user message、stop、MonitorDegraded 不受 300 秒退避限制：

   ```bash
   rg -n "terminal、user message、stop、MonitorDegraded 不受 300s 限制|必须立即处理" design/agent-long-task-suspension.md
   ```

7. 确认 human_tests 索引包含合并状态机和 5 分钟退避上限说明：

   ```bash
   rg -n "agent-long-task-suspension.md.*合并状态机.*5 分钟" human_tests/readme.md
   ```

**预期结果**：

- 所有 `rg` 命令退出码为 0。
- 设计文档明确状态机顶层只保留 `ActiveTurn`、`MonitoringExec`、`ResumingTurn`、`Terminal`。
- 设计文档明确 discovery、polling、watchdog、stall 都是 `MonitoringExec` 内部 substate 或 reason。
- 设计文档明确模型通知和 runtime poll/probe 的退避上限均为 300s，不会退避到 10 分钟、20 分钟。
- 设计文档明确 terminal/user/stop/MonitorDegraded 必须立即处理，不受退避上限影响。

**实际结果**：

- 通过。2026-05-25 按项目规则带 `source ~/.zshrc &&` 执行上述 7 条 `rg` 静态验收命令均成功；第 3 条命令使用单引号保护反引号，避免 zsh 命令替换。设计文档已将顶层状态收敛为 `ActiveTurn`、`MonitoringExec`、`ResumingTurn`、`Terminal`；discovery、scheduled polling、SilentRunning、StallSuspected、MonitorDegraded 均作为 `MonitoringExec` 内部 substate / reason；`Completed`、`Stopped`、`Timeout`、`WatcherLost`、`ProcessPruned` 统一通过 `terminal_state` 区分；模型通知退避和 runtime poll/probe 都有 300s 硬上限，terminal、user message、stop、MonitorDegraded 立即处理。`human_tests/readme.md` 已同步本文件 14 个用例计数。

### TC-ALT-15：Codex-style short grace、资源治理、长任务识别与输出唤醒静态验收

**操作步骤**：

1. 确认方案包含 Codex-style short grace 对照和 Codex 真实实现要点：

   ```bash
   rg -n "Codex-style short grace aggressive waiting|250ms..30000ms|MIN_EMPTY_YIELD_TIME_MS|collect_output_until_deadline|post-exit close wait|DEFAULT_EXEC_YIELD_TIME_MS|2500.*30000.*120000" design/agent-long-task-suspension.md
   ```

2. 确认 short grace 按调用面分预算，HTTP JSON 不会默认被 8-10 秒 grace 拖慢：

   ```bash
   rg -n 'HTTP JSON `/agent/chat`|2.5-5s|SSE/WebUI streaming|CLI/local exec|IM|short_grace_ms|channel_wait_budget|classification_confidence >= high' design/agent-long-task-suspension.md
   ```

3. 确认 short grace 是 notify-first/runtime wait，不会生成模型请求或无限延长：

   ```bash
   rg -n "output/exit/user/stop/deadline select|不发模型请求|不写 heartbeat history|quiet-window reset|总时间不得超过 channel cap|用户消息、stop、fatal stderr、process exit 永远抢占" design/agent-long-task-suspension.md
   ```

4. 确认资源治理不会静默 prune 或 silent kill 运行中任务，并且资源压力对模型可见：

   ```bash
   rg -n "禁止 silent prune|silent kill|ResourcePressure|ProcessPruned|ProcessDetached|MonitorDegraded|resource_pressure|silent_prune.*false|运行中用户可见任务默认不可静默 prune" design/agent-long-task-suspension.md
   ```

5. 确认资源治理有防 Token 黑洞和资源黑洞门禁：

   ```bash
   rg -n "资源压力 heartbeat 只更新 UI|不能每次 quota tick 都恢复模型|resume_seq 去重|5 分钟的 cooldown|backpressure|不得为了接收新任务静默杀掉旧 running task" design/agent-long-task-suspension.md
   ```

6. 确认长任务识别不是命令名、关键字、正则或工程路径猜测，而是综合显式策略、历史统计、运行时信号和调用面预算：

   ```bash
   rg -n "长任务识别策略|LongTaskClassification|ClassificationConfidence|ClassificationEvidence|模型/用户策略|历史统计|运行时信号|调用面预算|不能靠命令名、关键字、正则|不能内置特定命令名规则" design/agent-long-task-suspension.md
   ```

7. 确认输出唤醒策略区分 `OutputSignal`、output signal marker、阈值、progress-only 和 manual cursor：

   ```bash
   rg -n "输出唤醒判定|OutputSignal|output signal marker|OutputSeverity|progress_only|ready/final|大输出阈值|不唤醒模型的输出|model_visible_cursor" design/agent-long-task-suspension.md
   ```

8. 确认单元测试计划覆盖 short grace、分类、输出唤醒和资源治理：

   ```bash
   rg -n "short_grace_uses_channel_budget_and_high_confidence_only|short_grace_waits_on_notify_not_blind_sleep|long_task_classification_uses_runtime_signals_not_command_keywords|output_signal_progress_only_updates_ui_without_model_resume|running_task_is_never_silently_pruned_under_resource_pressure|resource_pressure_notice_is_deduped_and_rate_limited" design/agent-long-task-suspension.md
   ```

9. 确认 metrics 覆盖 short grace、classification、output signal 和 resource pressure：

   ```bash
   rg -n "long_task_short_grace_inline_complete_count|long_task_classification_count|long_task_output_signal_count|long_task_resource_pressure_count|long_task_resource_pressure_notice_suppressed_count" design/agent-long-task-suspension.md
   ```

10. 确认 human_tests 索引包含本用例覆盖点：

   ```bash
   rg -n "agent-long-task-suspension.md.*Codex-style short grace.*资源治理.*长任务识别.*输出唤醒" human_tests/readme.md
   ```

**预期结果**：

- 所有 `rg` 命令退出码为 0。
- 设计文档明确 short grace 参考 Codex 的 notify/deadline 机制，但不会让 HTTP JSON 普通 chat 固定等待 8-10 秒。
- 设计文档明确 running task 不允许 silent prune / silent kill；资源压力必须转为模型和用户可见的 structured event，并有去重和 cooldown。
- 设计文档明确长任务识别基于运行事实和多层证据，不使用命令关键字/正则/工程路径；unknown command 由 discovery/adaptive monitor 接管，不让模型反复空 poll。
- 设计文档明确 stdout/stderr 输出是否唤醒模型由 output signal marker、severity、阈值和 progress-only 判断，普通进度只进 UI。

**实际结果**：

- 通过。2026-05-25 按项目规则带 `source ~/.zshrc &&` 执行上述 10 条 `rg` 静态验收命令均成功。设计文档已补充 Codex-style short grace 对照：short grace 使用 notify/deadline 机制，不是 blind sleep；HTTP JSON 普通 `/agent/chat` 默认 `return_on_suspend`，只对 high-confidence short task 给 2.5-5s 小窗口，SSE/WebUI/CLI 可按通道预算更积极等待。设计文档已补充资源治理防黑洞规则：running task 不允许 silent prune / silent kill；资源压力只能降级为 detached、`ResourcePressure`、`ProcessPruned`、`ProcessDetached` 或 `MonitorDegraded` 等结构化状态，并通过 `resume_seq` 去重和 cooldown 防止 token 黑洞。设计文档已补充长任务识别策略和输出唤醒策略：分类综合显式策略、历史统计、运行时信号和调用面预算，不使用命令关键字/正则/工程路径；stdout/stderr 通过 `OutputSignal`、severity、阈值、progress-only 和 `model_visible_cursor` 判断是否唤醒模型，普通进度只进 UI。`human_tests/readme.md` 已同步本文件 15 个用例计数。

### TC-ALT-16：真实 Bifrost 服务长任务协作循环验收

**操作步骤**：

1. 构建并启动真实 Bifrost 服务，使用临时 `BIFROST_DATA_DIR` 和 `--no-system-proxy`，配置 mock OpenAI-compatible provider：

   ```bash
   e2e-tests/tests/test_agent_long_task_cooperative_loop.sh
   ```

2. 如已完成构建，可用现有 debug binary 复跑真实服务链路：

   ```bash
   SKIP_BUILD=true e2e-tests/tests/test_agent_long_task_cooperative_loop.sh
   ```

3. 确认脚本断言真实 `/api/im-gateway/agent/chat` 返回成功，且 mock provider 只收到两次模型请求：

   ```bash
   rg -n "len\\(payloads\\) == 2|model_request_count_while_waiting|process_exited|tool_name\"\\) for call in tool_calls" e2e-tests/tests/test_agent_long_task_cooperative_loop.sh
   ```

4. 确认脚本中的长任务命令不含用于分类的工程关键字，验证触发条件来自 running 状态而不是命令文本：

   ```bash
   ! rg -n "# cargo test|github-actions-pat|poll_run.py|gh run watch|scripts/ci/local-ci.sh" e2e-tests/tests/test_agent_long_task_cooperative_loop.sh
   ```

5. 确认脚本断言最终 tool call 只有 `exec_command`，没有模型驱动的 `write_stdin` 空轮询：

   ```bash
   rg -n 'assert \\[call.get\\("tool_name"\\) for call in tool_calls\\] == \\["exec_command"\\]' e2e-tests/tests/test_agent_long_task_cooperative_loop.sh
   ```

**预期结果**：

- 真实 Bifrost 服务启动成功，并通过 `/_bifrost/api/proxy/address` readiness。
- `/api/im-gateway/agent/chat` 真实请求完成，返回 `COOPERATIVE_LONG_TASK_LOOP_OK`。
- mock provider 请求数为 2：第一次模型请求发起 `exec_command`，第二次模型请求收到 runtime 压缩后的 `process_exited` tool result。
- 长任务等待期间 `model_request_count_while_waiting=0`。
- E2E 命令不包含工程关键字、分类注释或 GitHub Actions 专属脚本路径。
- 最终 tool call 列表只有 `exec_command`，没有 `write_stdin` 空轮询。

**实际结果**：

- 通过。2026-05-25 首次执行 `source ~/.zshrc && e2e-tests/tests/test_agent_long_task_cooperative_loop.sh` 完成真实 Bifrost 构建与启动，功能链路已到达 `COOPERATIVE_LONG_TASK_LOOP_OK`，但脚本断言误读 API 字段名：实际 JSON 中 tool call 输出字段为 `result`，脚本使用了 `output`，因此脚本自身失败。修复脚本后执行 `source ~/.zshrc && SKIP_BUILD=true e2e-tests/tests/test_agent_long_task_cooperative_loop.sh` 通过。CI 首次运行暴露 `SKIP_BUILD=true` 环境下脚本默认查找 `target/debug/bifrost`，而 shell E2E shard 使用预构建 `target/release/bifrost`；修复脚本的 binary resolver 后再次执行 `source ~/.zshrc && SKIP_BUILD=true e2e-tests/tests/test_agent_long_task_cooperative_loop.sh` 通过。2026-05-25 自适应修正后，脚本命令已移除 `# cargo test` 分类注释；重新构建当前源码后执行 `source ~/.zshrc && SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" e2e-tests/tests/test_agent_long_task_cooperative_loop.sh` 通过。真实服务链路验证了：长任务由真实 `exec_command` 执行，runtime watcher 基于 running 状态汇总 `LT_BEGIN` / `LT_DONE` 与 `process_exited`，等待期间 `model_request_count_while_waiting=0`，最终 API tool call 只有 `exec_command`。

### TC-ALT-17：Linux CI shell shard 中资源敏感 shell E2E 串行隔离回归

**操作步骤**：

1. 确认 `test_remote_shell_exec_streaming_e2e.sh`、`test_traffic_db_e2e.sh` 和 `test_openai_like_sse_search_e2e.sh` 已从并行 shell batch 降级到 isolated-after 串行队列：

   ```bash
   rg -n "test_remote_shell_exec_streaming_e2e.sh|test_traffic_db_e2e.sh|test_openai_like_sse_search_e2e.sh|Linux CI has observed these tests stall|ISOLATED_AFTER_TESTS" scripts/run_all_e2e.sh
   ```

2. 确认 shard 1 仍包含三个脚本，保证 CI 覆盖没有被跳过：

   ```bash
   BIFROST_E2E_SHARD_INDEX=1 BIFROST_E2E_SHARD_TOTAL=3 bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests | rg -n "test_remote_shell_exec_streaming_e2e.sh|test_traffic_db_e2e.sh|test_openai_like_sse_search_e2e.sh"
   ```

3. 使用当前已构建 binary 真实执行 remote shell streaming E2E：

   ```bash
   BIFROST_BIN="$PWD/target/debug/bifrost" SKIP_BUILD=true e2e-tests/tests/test_remote_shell_exec_streaming_e2e.sh
   ```

4. 使用当前已构建 binary 真实执行 traffic DB E2E：

   ```bash
   BIFROST_BIN="$PWD/target/debug/bifrost" SKIP_BUILD=true e2e-tests/tests/test_traffic_db_e2e.sh
   ```

5. 提交推送后观察 GitHub Actions 最新 `CI` run，确认 `E2E Shell (Linux, shard 1/3)` 不再在 resource-sensitive shell E2E 中卡到外层 per-test timeout。

6. 确认 Linux/macOS cleanup 等待 child process 使用有界 wait，避免 mock/SSE child 不退出时吞掉真实失败原因：

   ```bash
   rg -n "BIFROST_E2E_WAIT_PID_TIMEOUT|kill_pid_force|wait \\\"\\$pid\\\"" e2e-tests/test_utils/process.sh
   ```

7. 确认并行 shell batch 在支持 `setsid` 的平台用独立 process group 运行 child test，超时时杀 child process group 而不是只杀 wrapper pid：

   ```bash
   rg -n "setsid -w|pid_child_files|kill -TERM \"-\\$this_child_pid\"|kill -KILL \"-\\$this_child_pid\"" scripts/run_all_e2e.sh
   ```

**预期结果**：

- `scripts/run_all_e2e.sh` 明确将 `test_remote_shell_exec_streaming_e2e.sh`、`test_traffic_db_e2e.sh` 和 `test_openai_like_sse_search_e2e.sh` 放入 `ISOLATED_AFTER_TESTS`。
- shard 1 的测试列表仍包含三个脚本，覆盖没有被删除。
- 本地真实 E2E 通过：shell_text、argv_exec、stdin first-frame 三个 streaming regression 全部通过。
- 本地 traffic DB E2E 通过：query、updates、pending、detail、compact、sequence、pagination、filter、body retrieval、clear 全部通过。
- 本地 OpenAI-like SSE search E2E 通过：SSE traffic capture、response_body search、CLI search、invalid SSE fallback 和服务健康检查全部通过。
- `wait_pid` 在所有平台都使用有界等待；超时后强杀 child process，不再让 cleanup 阶段无限等待。
- 并行 shell batch 在 Linux 下为每个 child test 创建独立 process group；per-test timeout 可以杀整组 child，不再只杀 wrapper 进程；不支持 `setsid` 的平台应直接走 `env bash ...` fallback，不因 `set -u` 空数组展开失败。
- 远端 CI 不再因这些脚本在并行 batch 中启动真实服务、mock server、relay、caller 或 traffic generator 而卡到外层 per-test timeout。

**实际结果**：

- 通过。2026-05-25 按项目规则带 `source ~/.zshrc &&` 执行第 1、2、3、4、6、7 步均成功；本地 remote shell streaming 真实 E2E 结果为 Total 41、Passed 41、Failed 0；本地 traffic DB 真实 E2E 结果为 Total 10、Passed 10、Failed 0；本地 OpenAI-like SSE search 真实 E2E 结果为 Total 9、Passed 9、Failed 0。远端 CI 待最新提交完成后记录终态。

### TC-ALT-18：Traffic DB E2E mock server 端口 fallback 回归

**背景**：macOS CI shell shard 中 `test_traffic_db_e2e.sh` 曾在 Bifrost 启动后报 `Could not start mock server`。脚本先选择空闲 `MOCK_HTTP_PORT`，再启动 `http_echo_server.py`；如果端口在选择后、bind 前被抢占，echo server 支持 `--retries` fallback 到后续端口，但 Traffic DB 脚本仍轮询旧端口，导致假失败。

**操作步骤**：

1. 用 Python 临时占用将要传给脚本的 mock 端口，并在同一进程中启动 Traffic DB E2E，强制 `http_echo_server.py --retries 5` fallback：

   ```bash
   python3 - <<'PY'
import os
import socket
import subprocess

s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.bind(("127.0.0.1", 0))
try:
    port = s.getsockname()[1]
    env = os.environ.copy()
    env["MOCK_HTTP_PORT"] = str(port)
    env["SKIP_BUILD"] = "true"
    env["BIFROST_BIN"] = os.path.join(os.getcwd(), "target", "debug", "bifrost")
    subprocess.run(["bash", "e2e-tests/tests/test_traffic_db_e2e.sh"], env=env, check=True)
finally:
    s.close()
PY
   ```

2. 确认脚本输出包含 mock server fallback 提示：

   ```bash
   # 观察上一条命令输出中的：Mock server fell back from port <old> to <new>
   ```

3. 确认 Traffic DB E2E 的 10 个用例全部通过。

**预期结果**：

- `test_traffic_db_e2e.sh` 不再因为 mock server bind 前端口被抢占而失败。
- 脚本从 mock server 日志解析实际绑定端口，并用实际端口生成代理流量。
- Traffic Query、Updates、Pending、Detail、Compact、Sequence、Pagination、Filter、Body Retrieval、Clear 共 10 个用例全部通过。

**实际结果**：

- 2026-06-03 通过。执行上述 Python 端口抢占命令，脚本输出 `Mock server fell back from port 52155 to 52156`，Traffic DB E2E 汇总 `Total: 10`、`Passed: 10`、`Failed: 0`。

### TC-ALT-19：长任务等待期用户追加消息抢占并继续跟进原任务

**操作步骤**：

1. 确认设计文档记录了当前实现的用户消息抢占语义：

   ```bash
   rg -n '用户消息抢占|resume_reason=user_message|继续 `write_stdin` 跟进原 `session_id`' design/agent-long-task-suspension.md
   ```

2. 确认 turn loop 同时监听 guide notification 和 exec poll，并在用户消息到达时保留 deferred long task：

   ```bash
   rg -n "DeferredLongTask|guide_notification|resume_reason.*user_message|continue_deferred_long_task" crates/agent/src/session/turn_loop.rs
   ```

3. 执行定向单元回归：

   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-agent exec_command_long_task_user_message_interrupts_runtime_wait_then_continues -- --nocapture
   ```

4. 确认测试断言 guide 在长任务退出前触发第二次模型请求，并在回答追加问题后继续跟进原任务：

   ```bash
   rg -n "guide message should resume the model before the long task exits|model should handle the interjection, then continue following the long task|Continue following the original long task" crates/agent/src/session/tests.rs
   ```

**预期结果**：

- runtime watcher 等待 `exec_command` 长任务时，用户 guide/追加消息会抢占等待，不需要等 sleep/build/CI 结束。
- 原 `exec_command` tool call 先收到合法 tool result，内容包含 `resume_reason=user_message`、`running=true`、`session_id` 和等待期间的压缩输出。
- 模型先处理用户追加问题；处理完成后 turn loop 注入 runtime context，要求继续跟进原 `session_id`。
- 后续 `write_stdin` 能拿到原长任务最终输出，最终回复来自长任务完成后的模型轮次。

**实际结果**：

- 通过。2026-06-03 按项目规则带 `source ~/.zshrc &&` 执行第 1、2、4 步静态检查均成功；第 1 步使用单引号保护反引号，避免 shell 命令替换。执行 `source ~/.zshrc && SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-agent exec_command_long_task_user_message_interrupts_runtime_wait_then_continues -- --nocapture` 通过。测试使用真实 `exec_command(cmd="sleep 0.8; printf done", yield_time_ms=50)`，100ms 后向 `guide_channel` 注入“先回答我：2+2 等于几？”，断言第二次模型请求在 350ms 内出现，即没有等待 0.8s 长任务退出；模型先返回 `answered the interjection`，随后 runtime context 要求继续跟进原 `session_id`，模型调用 `write_stdin` 后最终输出包含 `done`，最终回复为 `long task complete`。

### TC-ALT-19：交互式 TTY prompt 卡住回归验收

**操作步骤**：

1. 确认 `exec_command` 对运行中的 TTY session 返回 `interactive` profile：

   ```bash
   rg -n 'suggested_wait_profile.*interactive|long_task_candidate = running|tty' crates/agent/src/tools/exec_command.rs
   ```

2. 确认 turn loop 不再排除 `tty=true`，且 interactive profile 使用较小 stall 阈值和 stdin 决策提示：

   ```bash
   rg -n "INTERACTIVE_LONG_TASK_PROFILE|INTERACTIVE_LONG_TASK_STALL_THRESHOLD|long_task_stall_hint|interactive terminal" crates/agent/src/session/turn_loop.rs
   ```

3. 执行交互式 prompt 单元回归：

   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-agent exec_command_tty_prompt_stall_returns_control_to_model_for_stdin_decision -- --nocapture
   ```

4. 确认测试模拟真实 PTY prompt、模型写 stdin 后再 poll，最终拿到确认输出：

   ```bash
   rg -n 'CONFIRM\?|ANSWER=yes|write_stdin|tty":true|interactive task complete' crates/agent/src/session/tests.rs
   ```

**预期结果**：

- `exec_command(tty=true)` 若 initial yield 后仍在运行，会成为 long task candidate，profile 为 `interactive`。
- runtime watcher 观察 PTY 输出和退出，但不自动写 stdin。
- PTY prompt 输出后短暂无新增输出时，tool result 包含 `resume_reason=stalled`、`profile=interactive`、`running=true`、`session_id` 和包含 `write_stdin` 决策建议的 hint。
- 模型可基于该 tool result 调用 `write_stdin` 输入确认；若 PTY 先回显输入，模型可继续空 poll，直到最终输出和 exit code 收敛。

**实际结果**：

- 通过。2026-06-03 按项目规则带 `source ~/.zshrc;` 执行第 1、2、4 步静态检查均成功；第 3 步执行 `source ~/.zshrc; SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-agent exec_command_tty_prompt_stall_returns_control_to_model_for_stdin_decision -- --nocapture` 通过。测试覆盖 `exec_command(tty=true)` 输出 `CONFIRM?` 后停止产出、runtime watcher 以 `profile=interactive` / `resume_reason=stalled` 归还模型、模型调用 `write_stdin("yes\n")`，并在 PTY 回显后继续 poll 到 `ANSWER=yes` 和最终回复 `interactive task complete`。

### TC-ALT-20：命令执行入口漏识别审查

**操作步骤**：

1. 确认本地 Agent tool registry 只注册一个 terminal 执行入口和一个 stdin 入口，且旧 shell alias 被拒绝：

   ```bash
   rg -n "exec_command|write_stdin|shell_command|local_shell|shell_pty|unknown_shell_aliases_are_rejected" crates/agent/src/tools/mod.rs
   ```

2. 确认 turn loop 的 long-task watcher 只接管 `exec_command` 的 structured running session，不把普通文件工具、plan 工具或 goal 工具误当 terminal：

   ```bash
   rg -n 'tool_name != "exec_command"|maybe_watch_exec_long_task|parse_long_task_candidate|ToolRegistry' crates/agent/src/session/turn_loop.rs
   ```

3. 确认 MCP tool 有 per-request timeout，不会无限卡住 Bifrost Agent turn；同时确认 MCP 不提供 Bifrost `write_stdin` 语义：

   ```bash
   rg -n "tool_timeout_sec|DEFAULT_TOOL_TIMEOUT_SEC|send_request_with_timeout|tools/call" crates/agent/src/mcp/mod.rs crates/agent/src/config.rs
   ```

4. 确认 IM external CLI runner 是外部进程边界：只写入初始 prompt，按 runner timeout 收敛，不能由内部 runtime watcher 代替外部 runner 处理 stdin 确认：

   ```bash
   rg -n "write prompt to external cli|wait_with_output|timeout_secs|external cli timed out|ExternalCliWorkerRun" crates/bifrost-admin/src/im_gateway/external_cli/mod.rs crates/bifrost-admin/src/im_gateway/external_cli/command_spec.rs
   ```

**预期结果**：

- 内部 Bifrost Agent 的 terminal session 全部走 `exec_command` / `write_stdin`，本次修复覆盖 pipe 和 PTY。
- 普通本地工具不会启动命令或等待 stdin；旧 shell alias 不可见且运行时拒绝。
- MCP tool 若阻塞会在配置的 `tool_timeout_sec` 后失败返回；需要 stdin 的交互不能靠 Bifrost runtime 注入，必须由对应 MCP 协议或工具自身处理。
- IM external CLI runner 可能运行很久，但它是外部 agent 进程；Bifrost 可停止/超时该 runner，不能进入外部 runner 内部 tool loop 去写 stdin。

**实际结果**：

- 通过。2026-06-03 按项目规则带 `source ~/.zshrc;` 执行上述 4 条静态审查命令均成功。审查结论：内部 Agent terminal 入口只有 `exec_command` / `write_stdin`，旧 shell alias 被拒绝；turn loop 只对 `exec_command` 的 structured running session 启用 watcher；MCP tool 通过 `send_request_with_timeout` / `tool_timeout_sec` 收敛但不具备 Bifrost `write_stdin` 语义；IM external CLI runner 以外部 runner 进程和 `timeout_secs` 为边界，内部 runtime watcher 不能替外部 runner 注入 stdin。

### TC-ALT-21：交互式长任务最终空 poll 不重复发起命令

**操作步骤**：

1. 对交互式长任务 E2E 脚本执行语法检查：

   ```bash
   bash -n e2e-tests/tests/test_agent_long_task_cooperative_loop.sh
   ```

2. 静态检查脚本消费 CI shell 调度器端口、mock server 绑定后回传真实端口，并用累计 tool transcript 判断交互输入已完成：

   ```bash
   rg -n 'BIFROST_PORT=.*ADMIN_PORT|PROXY_PORT|MOCK_PORT_FILE|server_address|E2E_ANSWER=yes.*joined_tools' \
     e2e-tests/tests/test_agent_long_task_cooperative_loop.sh
   ```

3. 使用已构建的 release binary 和外层注入端口真实执行长任务协作循环回归：

   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" \
     ADMIN_PORT=18141 PROXY_PORT=18141 \
     bash e2e-tests/tests/test_agent_long_task_cooperative_loop.sh
   ```

4. 复核本轮 CI 失败 artifact 原始根因：

   ```bash
   rg -n 'test_agent_long_task_cooperative_loop.sh|agent-long-task-interactive-loop|exceeded maximum iterations|E2E_ANSWER=yes' \
     /tmp/bifrost-ci-79358221612.log
   ```

**预期结果**：

- 语法检查通过。
- 静态检查显示脚本优先使用 `ADMIN_PORT` / `PROXY_PORT`，并由 Python mock server 绑定后写回真实端口。
- mock 模型在最终空 poll 只返回 `exit_code:0` 时，仍能从累计 tool transcript 看到上一条 `E2E_ANSWER=yes`，返回 `COOPERATIVE_INTERACTIVE_LOOP_OK`，不会重新调用 `exec_command`。
- 真实执行输出 `[agent-long-task-cooperative-loop] PASS`，覆盖普通长任务 runtime watcher 和交互式 TTY prompt 输入确认两条链路。

**实际结果**：

- 通过。2026-06-03 本轮执行：CI run `26900882610` 的 `E2E Shell (Linux, shard 2/3)` 失败日志显示 `test_agent_long_task_cooperative_loop.sh` 中 `agent-long-task-interactive-loop` 在最终空 poll 后重新发起交互命令，最终 `exceeded maximum iterations (4)`。修复后执行 `bash -n e2e-tests/tests/test_agent_long_task_cooperative_loop.sh` 通过；静态检查确认脚本消费 `ADMIN_PORT` / `PROXY_PORT`、mock server 动态回传端口，并用 `joined_tools` 判断 `E2E_ANSWER=yes`。随后执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" ADMIN_PORT=18141 PROXY_PORT=18141 bash e2e-tests/tests/test_agent_long_task_cooperative_loop.sh`，输出 `[agent-long-task-cooperative-loop] PASS`；真实执行使用临时数据目录、`--no-system-proxy` 与非 9900 端口。

## 清理步骤

1. 停止测试 Bifrost 进程。
2. 删除 `mktemp -d` 创建的临时数据目录。
3. 确认没有残留 mock model server、watcher 或测试 child process。
