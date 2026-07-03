# Agent Session Context Restore

## 背景

IM Gateway Agent 与 Web Agent Chat 会把会话历史以 append-only JSONL 事件流写入 `agent/sessions/<session_key>.jsonl`，并在服务重启后按 `session_state.json` 中的 `history_path` 恢复同一 session。历史恢复必须支持两条彼此正交的需求：

1. `total_tokens_used`：整个会话历史所有模型调用累计的 API 消耗，只用于成本/总量展示与 goal accounting。
2. `last_response_tokens`：最近一次模型请求返回时的 context 快照，用于 `effective_token_count()`、自动压缩阈值判断、`/status` 面板和 IM 卡片 Context 展示。

早期实现在恢复入口只回填了累计 token，用 `session_end.total_tokens = 50000` 之类值同时被写进 `session.last_response_tokens`，直接把累计消耗当作“当前 context”呈现，触发假阳性自动压缩，并让 Web/IM 状态面板显示 200%+ context 使用率。

同一恢复链路还有两个副作用：

- **状态误报**：`/agent/sessions/all` 用 `status:"active"` 表示 session 在内存中可打开，UI 却把它当成“正在跑”。正在执行 turn loop 的 session 会被 `AgentSessionManager::take_session()` 临时移出 idle map，需要单独维度区分。
- **External runner stop 不一致**：`/stop` 只触发内置 Agent 的 cooperative stop signal，没有写 external-cli stop marker，Codex/traex/chatgpt_web runner 不会停。

外加 CI 侧观察到的并发稳定性问题：Runner 以 `BIFROST_E2E_RUNNER_JOBS=8` 并发跑 e2e，部分修改进程级全局状态（`BIFROST_DATA_DIR`、mock provider env、Chat Gateway mock 计数）的用例会跨用例污染，尤其是 IM Gateway session persistence 相关用例。需要 serial-only 标记与阈值放宽双管齐下。

## 用户目标验证清单

### 必须实现

- 服务重启后同一 session_key 的 Chat 恢复：
  - `total_tokens_used` = JSONL 累计 `assistant_message.tokens` 之和（含 compaction post_tokens 语义）。
  - `last_response_tokens` = 最近一次 assistant_message 的 `context_tokens`（缺失回退 `tokens`）。
  - 若最近事件是 compaction 且带 `post_tokens`，用 `post_tokens` 覆盖 `last_response_tokens`。
- 恢复后 `session.restore_token_snapshot(runtime_state.last_response_tokens)` 把 snapshot boundary 设为当前 `history.len()`，追加消息才走增量估算。
- `/agent/sessions/all` 同时暴露 `status:"active"` 与 `running:bool`、`state:String`；只有 `running:true` 才是 turn loop 执行中。
- 列表实现合并 `AgentSessionManager::list_sessions()` 与 `list_active_turn_statuses()`，避免正在执行的 session（已被 `take_session()` 移出 idle map）从列表消失。
- IM `/stop` 与 `/agent/chat` `/stop` 共享 `request_agent_stop()` helper：先请求内置 Agent cooperative stop，再按 session_key 写 external-cli stop marker，让内置和 external runner 一致停止。
- WebUI Sessions 列表根据 `running` 字段展示 `Running` 或 `Active`，不再一律标 `Running`。

### 必须不破坏

- 老 JSONL 中只有 `tokens` 字段的 assistant_message 仍能恢复，`last_response_tokens` 回退到 `tokens` 值。
- `scan_session_summary()` 继续只累计 `tokens`，不重复累计 `context_tokens`。
- goal accounting、compaction 触发、CLI/Web status 命令行为不变。
- 未修改的正常 turn loop 路径不引入额外锁或 IO。
- external runner call 除 stop 外的行为不变（`run_started`、`run_finished`、result.json 语义均保留）。
- E2E runner 对不共享全局状态的用例仍并行执行；不引入无谓的全量串行降级。

### 必须真实验证

- 单元测试：runtime state 同时恢复累计 token 和最近 context 快照；compaction `post_tokens` 优先；`restore_token_snapshot()` 不让累计 token 进入 context；旧事件兼容。
- 单元测试：running turn 状态与 idle session 列表合并；`request_agent_stop()` 同时触发内置 stop 与 external-cli stop marker。
- E2E：`im_gateway_agent_chat_restores_history_after_service_restart` 重启恢复后 `/status` Context 使用最近响应快照。
- UI：Sessions 列表对 `running:false` 的 active session 展示 `Active` 而不是 `Running`。
- CI 并发 runner：`BIFROST_E2E_RUNNER_JOBS=8` 下 IM 相关 serial-only 用例稳定通过；`remote_shell_exec_streams_stdout` 首块阈值放宽后无假阳性。
- human_tests：`agent-session-persistence.md`、`im-gateway-external-cli-chat-gateway.md`、`ci-e2e-runner.md` 逐条执行。

## 产品语义

- **两类 token 分离**：`total_tokens_used` 与 `last_response_tokens` 具有完全不同的展示位与判定用途，恢复后不允许互相污染。
- **Active vs. Running**：`Active` 表示会话对象可打开；`Running` 只在 turn loop 真正执行时为真。恢复后的 idle session 是 `Active` 而非 `Running`。
- **/stop 是幂等且跨路径一致的**：无论是 IM 忙碌态 `/stop`、空闲态 `/stop`，还是 `/agent/chat` 的 `/stop`，都必须走同一 helper，同时通知内置 agent 与 external runner。
- **CI runner 并发隔离**：并发是加速手段，不是正确性保证；共享进程级状态的用例必须显式声明 serial-only。

## 技术细节

### 1. runtime state 恢复：`load_session_runtime_state()`

修改 `crates/agent/src/persistence.rs`：

- 保留 `scan_session_summary()` 计算累计 `total_tokens_used`。
- 在同一次遍历中记录：
  - 最近一条 `assistant_message` 的 `content.context_tokens`（缺失回退 `content.tokens`）作为候选 `last_response_tokens`。
  - 最近一条 `compaction` 事件的 `post_tokens`（若存在），优先覆盖 `last_response_tokens`。
- 返回 `RuntimeState { total_tokens_used, last_response_tokens }`。

关键实现点：

- JSONL 事件 append-only 且可能正在写；解析必须对末行 EOF/半行做 lossy skip。
- `scan_session_summary()` 保持只累计 `tokens`，避免重复累计 `context_tokens` 造成累计口径错乱。

### 2. session 恢复入口：`AgentSession::restore_from_runtime_state()`

所有入口（Web、IM Gateway、Admin API、chat gateway）在恢复 session 时：

```rust
session.total_tokens_used = runtime_state.total_tokens_used;
session.restore_token_snapshot(runtime_state.last_response_tokens);
```

`restore_token_snapshot()`（`crates/agent/src/session.rs`）：

- 设 `self.last_response_tokens = last_response_tokens`。
- 设 `self.last_response_history_len = last_response_tokens.map(|_| self.history.len())`。
- 后续 `effective_token_count()` 只在快照上追加尚未响应的新消息估算，不会把累计 token 当成当前 context。

### 3. `/agent/sessions/all`：Active + Running 分离

`crates/bifrost-admin/src/handlers/im_gateway/agent_api.rs`：

- 响应 item 增加 `running: bool` 与 `state: String`（如 `"running"`, `"idle"`, `"suspended"`）。
- 合并数据源：
  - `AgentSessionManager::list_sessions()`：内存 idle 中的 sessions。
  - `AgentSessionManager::list_active_turn_statuses()`：正在跑的 sessions（已 take_session）。
  - 按 session_key 合并，重复项以 running=true 覆盖。
- 状态优先级：`running > active(idle) > suspended`。

### 4. 统一 `/stop` helper：`request_agent_stop()`

`crates/bifrost-admin/src/handlers/im_gateway/utils.rs`：

```rust
pub async fn request_agent_stop(state: &AppState, session_key: &str) -> Result<StopSummary> {
    // 1. cooperative stop signal for built-in Agent turn loop
    let inflight = state.agent_session_manager.request_stop(session_key).await;
    // 2. external-cli stop marker for runner-driven turns
    let marker = state.external_cli.write_stop_marker(session_key).await?;
    Ok(StopSummary { inflight, marker })
}
```

调用点：

- `POST /_bifrost/api/im-gateway/agent/chat` 的 `/stop`。
- IM 忙碌态 `/stop`、空闲态 `/stop`。
- Web `AgentChatSection` `/stop`。

### 5. WebUI 状态展示

- `web/src/pages/AI/AgentChatSection.tsx`：读取 `running` 字段决定 `Running` 或 `Active` 徽章文案与颜色。
- `web/src/pages/Settings/tabs/agent/UnifiedSessionsSection.tsx`：列表 tag 同步。
- 悬停 tooltip 说明 `Running`/`Active` 差异，减少用户困惑。

## CLI / Admin API / Web

### CLI

- `bifrost agent status --session <key>` 输出：
  - `state: running|active|suspended`
  - `context_tokens: <last_response_tokens>`
  - `total_tokens_used: <累计>`
- `bifrost agent stop <session>` 复用 `request_agent_stop()` helper。

### Admin API

- `GET /_bifrost/api/im-gateway/agent/sessions/all` 返回：
  ```json
  {
    "sessions": [
      {
        "session_key": "im:chat:xxx",
        "status": "active",
        "running": true,
        "state": "running",
        "context_tokens": 12800,
        "total_tokens_used": 91300
      }
    ]
  }
  ```
- `POST /agent/sessions/{key}/stop`：调 `request_agent_stop()`。

### Web

- Sessions 列表：`Running` 高亮，`Active` 淡化。
- Chat 页头 status 徽章按同一字段展示。

## Sync 边界

- Runtime state 与 JSONL 属于本机数据目录，不跨设备 sync。
- external-cli stop marker 落地在 chat_runs 目录，同样本机。

## 实现切分

### Phase 1：runtime state 恢复口径

- 修改 `load_session_runtime_state()` 恢复 `last_response_tokens`。
- 更新 `AgentSession::restore_token_snapshot()`。
- 修改所有恢复入口（Web、IM、Admin API）调用 `restore_token_snapshot()`。
- 单元测试覆盖累计 vs. 快照分离、compaction post_tokens 优先、旧事件兼容。

### Phase 2：Active vs. Running

- 修改 `/agent/sessions/all` 响应结构与合并逻辑。
- WebUI Sessions 列表按 `running` 字段展示。
- 单元测试覆盖列表合并、running 优先。

### Phase 3：统一 stop helper

- 提炼 `request_agent_stop()`；所有入口切过去。
- 单元测试覆盖 stop marker + inflight signal。
- E2E：external runner 场景下 `/stop` 真的能停。

### Phase 4：CI E2E runner 并发稳定性

- `TestCase` 增加 `parallel_safe` 标记与 `serial()` builder；默认 `parallel_safe = true`。
- `run_all_parallel()` 先并行 parallel_safe 用例，再串行 serial-only 用例。
- 标记 `im_gateway_agent` 与 `im_gateway_session_persistence` 为 serial-only。
- 放宽 `remote_shell_exec_streams_stdout` 首块 stdout 阈值到 1000ms，保留“首块早于第二块 + 分片完整”语义。
- 编译验证与真实 8 并发复跑。

### Phase 5：文档与 human_tests

- 更新 `human_tests/agent-session-persistence.md`、`human_tests/im-gateway-external-cli-chat-gateway.md`。
- 新增 `human_tests/ci-e2e-runner.md`。
- 更新 `human_tests/readme.md` 索引。

## 测试方案

### 单元测试

- `persistence::runtime_state_recovers_context_snapshot_separately_from_total`：
  - JSONL 含 3 条 assistant_message，累计 tokens 90000，最近一条 context_tokens=8000。
  - 恢复后 `total_tokens_used=90000`、`last_response_tokens=Some(8000)`。
- `persistence::compaction_post_tokens_wins_over_assistant_context`：
  - 最近事件是 compaction post_tokens=3000，覆盖 assistant context_tokens=8000。
- `persistence::runtime_state_falls_back_to_tokens_for_old_events`：
  - 老事件只有 `tokens=1200`，恢复 `last_response_tokens=Some(1200)`。
- `session::restore_token_snapshot_sets_history_boundary`：
  - 恢复后 `effective_token_count()` 只对 snapshot 之后的追加消息估算。
- `agent_api::sessions_all_merges_running_and_idle`：running 与 idle 合并，running 优先。
- `utils::request_agent_stop_signals_inflight_and_marker`：验证同时触发 cooperative stop 与 stop marker 写入。
- `e2e_runner::parallel_safe_default_true_and_serial_only_runs_sequential`。

### E2E 测试

- `crates/bifrost-e2e/src/tests/im_gateway_session_persistence.rs::im_gateway_agent_chat_restores_history_after_service_restart`：
  - 重启后 `/status` Context 使用最近响应快照，不再是累计 token。
  - `total_tokens_used` 与最近响应差距明显（>10x），断言两者不相等且分别有正确数值。
- `im_gateway_external_runner_stop_triggers_marker`：external runner 场景下 `/stop` 后 result.json 状态为 stopped。
- CI 并发：以 `BIFROST_E2E_RUNNER_JOBS=8` 复跑 4 个历史失败用例，验证 serial-only 后稳定通过。
- `remote_shell_exec_streams_stdout`：首块 stdout < 1000ms 且第二块晚于第一块，分片完整。

### 真实场景测试（human_tests）

- 更新 `human_tests/agent-session-persistence.md`：
  - TC-ASP-01：JSONL 恢复后 Context vs. Total 不再混用。
  - TC-ASP-02：Sessions 列表 idle session 展示 `Active`。
  - TC-ASP-03：Running turn 用例展示 `Running`，`/stop` 生效。
- 更新 `human_tests/im-gateway-external-cli-chat-gateway.md`：
  - TC-IMEX-04：`/stop` 触发 Codex/traex/chatgpt_web runner 停止。
- 新增 `human_tests/ci-e2e-runner.md`：
  - TC-CIER-01：`BIFROST_E2E_RUNNER_JOBS=8` 下 IM 相关 serial-only 用例稳定。
  - TC-CIER-02：`remote_shell_exec_streams_stdout` 阈值回归。

启动 Bifrost 必须使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-agent persistence session restore`
- `cargo test -p bifrost-admin im_gateway agent_api utils`
- `cargo test -p bifrost-e2e --no-run`
- 相关 E2E 脚本按需执行
- `cargo test --workspace --all-features`
- `rust-project-validate`
- 本机 no-local-coverage 生效时不跑 `make coverage`；交付时说明。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核恢复入口是否全部调用 `restore_token_snapshot()`，无遗漏。
- 复核 `/agent/sessions/all` 是否同时合并 idle + running。
- 复核 `/stop` 相关入口是否统一走 helper。
- 复核 e2e-runner 是否只对必要用例串行。
- 运行受影响单元与 E2E；修复 assertion 遗漏。

### 第 2 轮

- 复查 `git diff` 与 human_tests 索引。
- 复查 WebUI Sessions 列表 UI 文案与颜色。
- 再复跑 CI 并发 runner，观察是否仍有跨用例污染。
- 若失败再定位是并发污染、阈值抖动还是功能缺陷，分别处置。

## 风险与决策点

- **老 JSONL 兼容**：只 fallback 到 `tokens` 字段；若某些老事件既无 `tokens` 也无 `context_tokens`，恢复后 `last_response_tokens=None`，`effective_token_count()` 回退到 estimate。可接受。
- **累计 vs. 快照混淆**：改口径后如果某些 provider 只返回 `total_tokens`，`context_tokens` 会回退到 total；仍比“累计 token 直接进入 context”更接近真实。见 `agent-token-usage.md` 的 `TokenUsage::context_tokens()`。
- **合并 idle/running 列表的性能**：`list_active_turn_statuses()` 是 in-memory 数据，成本可忽略；未来若 running 集合变大，可缓存。
- **Serial-only 用例扩散**：仅对真正修改全局状态的用例标记 serial；避免全量降级导致 CI 时间劣化。
- **Runner stop 语义**：external runner 是否真的响应 stop marker，取决于 adapter 自己的等待循环。相关下沉修复见 `agent-run-timeline-channel-unification.md` 中的 External Runner SSE 停止一致性方案。
