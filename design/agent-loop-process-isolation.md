# Agent Loop 进程隔离

## 背景

Bifrost 主进程同时承载：

- 流量代理（`crates/bifrost-proxy`）。
- Admin API / WebUI 后端（`crates/bifrost-admin`）。
- IM Gateway（`crates/bifrost-admin/src/im_gateway/*`）。
- Agent / Runner 调度：内置 Bifrost Agent loop（`crates/agent/src/session/turn_loop.rs`）、IM Agent loop、Web/Admin 非流式测试入口、内置 runner-call、外置 Runner（ChatGPT Web、Codex CLI、自定义 CLI Runner）。

旧实现所有 Agent/Runner 执行域都跑在主进程 Tokio runtime 内：`run_turn_with_mcp_multimodal()` 是 Tokio task；`ExternalCliRuntime::run()` 编排 Codex CLI 子进程但 orchestration 循环仍在主进程；ChatGPT Web adapter 的浏览器 / CDP 自动化也在主进程执行。任一 CPU 密集 / 同步阻塞 / 浏览器卡住都会挤占主 runtime，代理流量、Admin API、IM 消息处理和 `/stop` 一起失去响应。

`/stop` 依赖同进程 cooperative `AgentStopSignal`，只有 loop 回到 await/checkpoint 才生效，不能强制中断同进程卡死代码。根因是执行域和控制域共享主进程；必须把执行域迁移到可独立终止的子进程。

本方案把默认 Agent / Runner 执行改为“每个会话一个独立进程”：主进程只负责接收用户输入、维护会话 busy/preview 状态、转发进度事件和返回最终结果；内置 Bifrost Agent 与外置 Runner 都在子进程中完成实际 loop / 编排。控制面（`AgentSessionManager` busy gate、`/stop`、guide queue、SSE 转发、message log store、schedule store 等）仍留在主进程。

## 用户目标验证清单

### 必须实现

- 内置 Bifrost Agent 每次 `run_turn_with_mcp_multimodal()` 默认在独立子进程（`bifrost agent worker`）中运行。
- 外置 Runner（Codex CLI、ChatGPT Web、自定义 CLI Runner）每次 `ExternalCliRuntime::run()` 默认在独立子进程（`bifrost agent external-runner-worker`）中运行。
- 主进程 `/stop` 汇总控制 internal cooperative signal、内置 Agent worker、外置 Runner worker、legacy external CLI run stop；worker 超时后 kill 整个 process group。
- SSE 客户端断开或用户显式 `/reset` 后 worker 被清理，不产生孤儿进程。
- Worker 写入的 `send_msg` 消息、`schedule` 定时任务、JSONL history、run_state、work_dir 等必须落盘后可被主进程立即读回，`api/list` 不会读到旧内存快照。
- `exec_command` pipe 模式在 Unix 下用独立 process group 启动 shell；session terminate 与 session drop 会先终止整个 process group，再兜底 kill 直接 child，避免 `zsh -ic -> python` 等孙进程孤儿。
- 生产环境 worker 启动失败时 fail closed（返回明确错误 + 释放 active session），不再静默回退到主进程执行 Agent loop。
- 测试环境保留 in-process worker fallback：未设置 `BIFROST_FORCE_AGENT_WORKER` 时使用进程内 worker 以保持 unit test 可控。
- IM built-in Agent guide queue 在 worker 运行期间必须被主进程实时唤醒并转发；不能等到 worker 下一个 event 才处理。
- Worker 恢复 JSONL history 时必须同步 runtime state（plan、goal、token snapshot、compaction count、base instructions、原始 source channel）。

### 必须不破坏

- 不改变已有 Agent 用户输入格式：Web/Admin `/api/agent/chat/stream`、IM `/_bifrost/api/im-gateway/agent/chat`、`/api/agent/chat/completions` 契约保持一致。
- 不改变 `AgentSessionManager` 的 busy gate / session preview 语义。
- 不改变 `AgentStopSignal` cooperative 接口；worker stop 附加而不是替代。
- 不破坏 CI / E2E 中 `bifrost-e2e` 作为 in-process Admin 服务的运行方式；`current_exe()` 启动 worker 时 `bifrost-e2e` 必须支持 hidden `agent worker` / `agent external-runner-worker` pass-through 入口。
- 不改变现有 external CLI runtime 对 Codex / ChatGPT Web / 自定义 CLI 的编排语义，只是把 orchestration 迁进 worker。
- 不影响 tray / cert / rules / traffic 等非 Agent 路径。

### 必须真实验证

- 单元测试覆盖：worker request 协议版本、`TurnResult` roundtrip、协议不兼容拒绝、`BIFROST_FORCE_AGENT_WORKER` 环境语义、worker 启动失败 fail-closed、`exec_command` process group 清理。
- E2E 验证：真实 `bifrost` 二进制启动服务后触发 Web/Admin 内置 Agent 请求，确认独立 worker 进程出现、主进程 Admin API 继续响应；`/stop` 停止 worker；SSE 断开清理 worker；外置 Runner worker 同样验证。
- CI shell E2E worker 隔离回归：`send_msg`、`schedule`、history、run_state 落盘后主进程 API 立即可读；guide 队列被 worker event 阻塞时仍能唤醒转发。
- human_tests 覆盖真实 Web / IM / Admin 场景的用户可感知：`/stop` 有效、SSE 断开不留孤儿、启动失败有明确错误。

### 必须交付

- 更新 `crates/bifrost-admin/src/im_gateway/agent_worker.rs`、`crates/bifrost-admin/src/im_gateway/external_cli/mod.rs`、`crates/bifrost-cli/src/cli.rs`（hidden 子命令 `Worker` / `ExternalRunnerWorker`）、`crates/bifrost-admin/src/handlers/agent_chat.rs`、`crates/bifrost-admin/src/handlers/im_gateway/agent_chat.rs`、`crates/bifrost-admin/src/handlers/im_gateway/agent_api.rs`。
- 更新 E2E 与 human_tests；更新 `human_tests/readme.md` 索引。
- 完成至少两轮 Review/Fix/Test 闭环。

## 卡死原因分析

- 旧实现把 `run_turn_with_mcp_multimodal()` 放在主进程 Tokio task 中运行，`/stop` 依赖同进程 cooperative `AgentStopSignal`。
- 外置 Runner 虽然启动 Codex 等 CLI 子进程，但 `ExternalCliRuntime::run()` 本身以及 ChatGPT Web adapter 的浏览器 / CDP 自动化仍在主进程中执行。
- 一旦 loop 内部发生 CPU 密集、同步阻塞或浏览器自动化卡住，主进程 runtime 被饥饿，代理请求和 `/stop` HTTP/IM 事件也排不上执行。
- 即使 `/stop` 被处理，旧内存信号也只能在 loop 回到 await/checkpoint 后生效，不能强制中断同进程卡死代码。
- 根因是执行域和控制域共享主进程；必须把执行域迁移到可独立终止的子进程。

## 产品语义

### Worker 是执行域，主进程是控制域

- 主进程：接收用户输入、维护 busy gate、维护 session preview、转发 SSE / IM 卡片、处理 `/stop`/`/reset`/`/clear` 等控制命令、跨会话调度、message log、schedule store、run_state。
- Worker：执行 turn loop、工具调用、MCP、浏览器 / CDP 编排、CLI Runner orchestration。

Worker 崩溃或超时不影响主进程；主进程升级不打断 in-flight worker（worker 仍完成当前 turn，主进程可选择放弃收集结果）。

### 控制信号仍是 cooperative + kill

- `/stop` 首先通过 stdio 发送 `Stop` 命令给 worker（`WORKER_STOP_GRACE_MS = 1500ms`）；worker 内部信号回收 cooperative stop。
- 超时后主进程用 `kill(pid, SIGTERM)` -> `kill(pid, SIGKILL)` 终止 worker process group。
- Legacy external CLI run stop 仍走原路径（`ExternalCliRuntime::stop_by_run`），worker 内部会同时清理自身启动的 external CLI 子进程。

### fail closed vs. in-process fallback

生产：worker 可执行文件不可启动、协议版本不兼容、stdio 握手失败时，`AgentWorkerClient::spawn_or_fallback()` 返回错误，主进程释放 active session 并对调用方返回 `AGENT_WORKER_START_FAILED`，不再静默走进程内 loop。

测试：未设置 `BIFROST_FORCE_AGENT_WORKER` 时使用 in-process worker，保持单元测试可控；单元 `spawn_or_fallback_uses_in_process_worker_in_tests_without_force_env` 与 `spawn_or_fallback_fails_closed_when_forced_worker_cannot_start` 守住两条路径。

## 技术细节

### 关键 struct / const / API

`crates/bifrost-admin/src/im_gateway/agent_worker.rs`：

```rust
const WORKER_PROTOCOL_VERSION: u32 = 1;
const WORKER_STOP_GRACE_MS: u64 = 1500;
const WORKER_OUTPUT_CLOSED: &str = "__bifrost_agent_worker_output_closed__";

static ACTIVE_WORKERS: DashMap<String, AgentWorkerStopHandle>;

pub struct AgentWorkerRunRequest {
    pub protocol_version: u32,
    pub session_key: String,
    pub message: String,
    pub config: Option<AgentConfig>,
    pub images: Vec<ChatImageInput>,
    pub queued_messages: Vec<String>,
    pub guide_messages: Vec<String>,
    pub system_prompt: Option<String>,
    pub collaboration_mode: Option<CollaborationMode>,
    pub work_dir: Option<String>,
    pub history_path: Option<String>,
    pub source: Option<String>,
    pub default_message_channel: Option<ImMessageChannelBinding>,
    pub agent_proxy_port: Option<u16>,
}

pub struct AgentWorkerRunResult {
    pub response: String,
    pub tool_calls_log: Vec<ToolCallLog>,
    pub work_dir_switched: Option<String>,
    pub title_updated: Option<String>,
    ...
}

pub enum AgentWorkerEvent {
    Started { pid: u32 },
    Progress { event: AgentTurnProgressEvent },
    Finished { result: Box<AgentWorkerRunResult> },
    Failed { error: String },
    Stopped,
}

pub struct AgentWorkerClient { executable: PathBuf }

impl AgentWorkerClient {
    pub async fn spawn_or_fallback(request: AgentWorkerRunRequest) -> Result<AgentWorkerRun, String>;
    async fn spawn_or_fallback_with_client(client: Self, request: AgentWorkerRunRequest) -> ...;
}
```

`crates/bifrost-cli/src/cli.rs`：

```rust
enum AgentSubcommand {
    ...
    #[command(hide = true, about = "Run isolated built-in agent worker over stdio")]
    Worker,
    #[command(hide = true, about = "Run isolated external runner worker over stdio")]
    ExternalRunnerWorker,
}
```

`crates/bifrost-admin/src/im_gateway/external_cli/mod.rs`：

```rust
pub struct ExternalCliRuntime { ... }

impl ExternalCliRuntime {
    pub async fn run(&mut self, ...) -> Result<...>;
    // worker path
    async fn spawn_external_cli_worker_process(executable: &Path) -> Result<Child>;
    async fn write_external_cli_worker_command(stdin: &mut ChildStdin, cmd: &ExternalCliWorkerCommand);
    fn send_external_cli_worker_event(event: &ExternalCliWorkerEvent) -> Result<()>;
}

enum ExternalCliWorkerEvent {
    Started { pid: u32 },
    Progress { event: ExternalRunnerProgressEvent },
    Finished { result: Box<ExternalRunnerResult> },
    Failed { error: String },
    Stopped,
}
```

`current_exe()` 决定 worker 使用哪个二进制。生产 = `bifrost`；`bifrost-e2e` in-process Admin 场景 = `bifrost-e2e` 自身，需要在 `bifrost-e2e` 里支持 hidden `agent worker` / `agent external-runner-worker` pass-through，运行时把 `BIFROST_EXTERNAL_CLI_WORKER=1` 或 worker stdio 协议交回给真实 runtime。

### Worker 协议

stdio NDJSON：

- Client -> Worker：`{"cmd":"Run", "request": <AgentWorkerRunRequest>}` / `{"cmd":"Guide","message":"..."}` / `{"cmd":"Stop"}`。
- Worker -> Client：`{"event":"Started","pid":123}` / `{"event":"Progress","event":<AgentTurnProgressEvent>}` / `{"event":"Finished","result":<AgentWorkerRunResult>}` / `{"event":"Failed","error":"..."}` / `{"event":"Stopped"}`。
- Worker 侧解析请求时严格校验 `protocol_version == WORKER_PROTOCOL_VERSION`；不兼容返回 `Failed`。
- Worker 内部若探测到 stdout pipe close，写入 `WORKER_OUTPUT_CLOSED` 标记后主动退出。

### 进程组隔离

Unix：worker 与 `exec_command` 都使用 `process_group(0)` 启动；stop 时对整个 process group 发信号。Windows：使用 job object / `CREATE_NEW_PROCESS_GROUP` 等价语义。

`exec_command` pipe：`Ctrl-C`、session terminate、session drop 都先终止整个 process group，再兜底 kill 直接 child。测试 `exec_command_ctrl_c_terminates_pipe_process_group_children` 守住。

### 主进程与 worker 状态同步

`AgentSessionManager` busy gate 保留在主进程。worker `Progress` 反向同步 active turn status、title、plan/progress card；主进程收到 `Started { pid }` 后把 `AgentWorkerStopHandle` 挂入 `ACTIVE_WORKERS`；`Finished` / `Failed` / `Stopped` / stdio EOF 都触发清理。

Worker 与主进程各自持有 `ImGatewayService`；`send_msg` / `schedule` 工具写入的消息与定时任务落盘后，主进程读取 store 前必须重新加载磁盘数据（`message_log_store::reload_from_disk()`、`schedule_store::reload_from_disk()`），避免独立进程写入后 API/list 仍读到旧内存快照。

Worker 恢复 JSONL history 时使用 `bifrost_agent::persistence::ConversationRecorder::replay_full()`，同步恢复 `runtime_state.current_plan`、`goal`、`token_snapshot`、`compaction_count`、`base_instructions`、`source`。避免续聊后 `/status`、plan、timeline run_state 与主进程恢复路径不一致。

### Guide queue 唤醒

IM built-in Agent 主进程在等待 worker event 时必须同时 `select!` 监听 guide channel notification。收到 guide 后立即 `write_stdin` 发 `Guide` 命令给 worker，不要等 worker 产生下一条 event 才转发。相关代码路径：`crates/bifrost-admin/src/handlers/im_gateway/agent_chat.rs:1797 forward_guide_message`。

### `/stop` / `/reset` / `/clear` 控制路径

- `POST /_bifrost/api/im-gateway/agent/chat` 的 `/stop`：session-free 控制路径，聚合 internal cooperative signal + worker stop + legacy external CLI stop，返回 `200 { stopped: true }`，不把 worker stopped 当作 500。
- `/reset` / `/clear`：session-free 控制路径，停止 active worker、清理内存 session/queue，删除 built-in Agent adapter 持久化 session state 与 JSONL history，服务重启后不恢复旧上下文。
- `POST /_bifrost/api/agent/chat/stream` 的 SSE 断开：主进程通过 `on_disconnect` 触发 `AgentWorkerStopHandle::stop_tx.send(Stop)` 并 kill worker process group。

## CLI / Web / Admin API

### CLI

- `bifrost agent worker`：hidden 子命令，stdin/stdout 承担 worker 协议；不出现在 `bifrost --help`。
- `bifrost agent external-runner-worker`：hidden 子命令，承载 external CLI runtime。
- `bifrost agent status`：显示 active worker 数、每个 worker 的 pid / session_key / uptime，帮助运维排查孤儿。
- `bifrost agent stop <session_key>`：主进程 CLI，通过 Admin API 转 `/stop`。
- Windows / Linux / macOS：`--relay-remote` 场景对 remote invoke 的 worker 转发透明，不新增额外 flag。

### Web / Admin

- Web `Agent` 页面若发起流式 chat，SSE 首帧 `worker.started { pid }`，方便前端调试。前端不必展示 pid，但可在 dev tools console 输出。
- `/settings/agent` 增加“worker 进程隔离”开关的说明文本（对生产环境永远开启，测试环境可通过环境变量控制）。
- Admin `/api/agent/workers` 返回：

  ```json
  [{ "sessionKey": "...", "pid": 12345, "kind": "builtin", "startedAt": "...", "workDir": "..." }]
  ```

### Admin API

- `POST /_bifrost/api/agent/chat/stream`：内部走 `AgentWorkerClient::spawn_or_fallback`；SSE 事件包含 `worker.started` / `worker.progress` / `worker.finished` / `worker.failed` / `worker.stopped`。
- `POST /_bifrost/api/im-gateway/agent/chat`：同上；非流式测试入口也走 worker。
- `POST /_bifrost/api/im-gateway/agent/chat/stop` / `/reset` / `/clear`：session-free 控制路径，返回 `200`。
- `GET /_bifrost/api/agent/workers`：诊断接口，返回 active worker 列表。
- `POST /_bifrost/api/agent/workers/:pid/stop`：仅在诊断场景使用；正常走 session-level stop。
- Chat Gateway、IM Event Loop、Schedule、Daily Agent 等所有 `ExternalCliRuntime::run()` 调用默认进入外置 Runner worker，通过同一 `agent-workers` 诊断接口暴露。

## Sync 边界

- Worker 生命周期属于本机 runtime，不参与远端 rule / group / value sync。
- `AgentSessionManager` busy / preview 属本地状态，不同步。
- JSONL history、`send_msg` message log、schedule store 均属本地持久化；`crates/bifrost-sync` 不上传这些数据。
- Remote invoke（bifrost-remote）在目标端启动 worker 时使用目标机的 `current_exe()`，不通过 relay 传递 worker 进程；relay 只承担 stdio 转发。
- 多设备协作场景不共享 worker；每台机器独立启动。

## 实现切分

### Phase 1：内置 Agent worker

- 新增 hidden `bifrost agent worker` 子命令。
- 抽 `AgentWorkerRunRequest` / `AgentWorkerRunResult` / `AgentWorkerEvent`；worker 内部执行 `run_turn_with_mcp_multimodal()`、MCP、`send_msg` / `schedule` 工具注册、goal continuation。
- Unix 独立 process group；stop 超时 kill worker process group。
- `AgentWorkerClient::spawn_or_fallback` + `BIFROST_FORCE_AGENT_WORKER` 环境语义。
- 单元测试守协议、fail-closed、in-process fallback。

### Phase 2：覆盖入口

- Web/Admin `/api/agent/chat/stream` 使用内置 Agent worker。
- IM built-in Agent `process_agent_chat()` 使用内置 Agent worker；主进程 guide queue 转发 worker `Guide` 命令；等待 event 时同时监听 guide channel notification。
- Web/Admin `/api/im-gateway/agent/chat` 非流式测试入口使用内置 Agent worker。
- `bifrost-e2e` in-process E2E 服务支持 hidden `agent worker` / `agent external-runner-worker` pass-through 入口，避免 in-process E2E 服务把 worker 子进程误启动为普通测试 runner。
- Slash runner-call 目标为 built-in Agent 时使用内置 Agent worker。

### Phase 3：外置 Runner worker

- 新增 hidden `bifrost agent external-runner-worker` 子命令。
- `ExternalCliRuntime::run()` 默认先启动 worker 子进程；只有 worker 内部通过 `BIFROST_EXTERNAL_CLI_WORKER=1` 执行原始 runtime。
- Codex / 自定义 CLI 的命令启动、ChatGPT Web adapter 的浏览器 / CDP 编排均迁移到 worker 内。
- worker stop 先发 stdio stop，再超时 kill worker process group；worker 内 stop 会清理自身已启动的 external CLI 子进程。
- Chat Gateway、IM Event Loop、Schedule、Daily Agent 等所有 `ExternalCliRuntime::run()` 调用默认进入外置 Runner worker。

### Phase 4：状态同步、事故加固与文档

- 主进程读取 message log / schedule store 前 reload from disk。
- worker 恢复 JSONL history 时同步 runtime state。
- `/stop` 聚合 internal cooperative signal + 内置 Agent worker + 外置 Runner worker + legacy external CLI run stop。
- `/reset` / `/clear` 走 session-free 控制路径；删除持久化 session state 与 JSONL。
- SSE 断开或 stop 后清理 worker，避免孤儿进程。
- `exec_command` pipe 模式 Unix 独立 process group；Ctrl-C、session terminate、session drop 都先终止整个 process group。
- 生产环境 worker 启动失败 fail closed，返回明确错误并释放 active session。
- 更新 `human_tests/agent-loop-process-isolation.md` 与 `human_tests/readme.md` 索引；更新 `AGENTS.md` / `docs/` 中 Agent worker 相关章节。

## 测试方案

### 单元测试

- `agent_worker::build_run_request_uses_protocol_version_and_session`：验证 worker 请求携带协议版本、session、work_dir、source。
- `agent_worker::turn_result_roundtrip_preserves_stop_fields`：验证 worker result 与 `TurnResult` 转换不丢字段。
- `agent_worker::validate_request_rejects_bad_protocol`：验证协议版本不兼容时拒绝。
- `agent_worker::spawn_or_fallback_uses_in_process_worker_in_tests_without_force_env`：验证仅测试环境允许未强制 worker 时使用进程内 fallback。
- `agent_worker::spawn_or_fallback_fails_closed_when_forced_worker_cannot_start`：验证 worker 可执行文件不可启动时返回错误，不进入主进程 loop。
- `exec_command_ctrl_c_terminates_pipe_process_group_children`：验证 pipe exec session 终止时后台孙进程也被同组清理。
- `external_cli` targeted tests：验证 external CLI runtime、stop by run/session、Codex adapter、IM event loop external runner 仍通过；测试环境默认绕过 worker，真实 E2E 覆盖 worker 进程。

### E2E 测试

`e2e-tests/tests/test_agent_worker_process_isolation.sh`：

- 构建当前 bifrost。
- 用临时 `BIFROST_DATA_DIR` 启动服务，必须带 `--no-system-proxy`。
- 配置内置 Agent 慢速 mock 模型，发起 `/api/agent/chat/stream`。
- 断言独立 `bifrost agent worker` 子进程出现，主进程 Admin API 继续响应。
- 调用 `/stop`，断言 worker 退出且主进程继续响应。
- 断开 SSE，断言 worker 自动清理。
- 配置 slow mock external runner，发起 `/api/im-gateway/chat/stream`。
- 断言独立 `bifrost agent external-runner-worker` 子进程出现，主进程继续响应，`/stop` 可停止外置 Runner worker。

`cargo run -p bifrost-e2e -- --test im_gateway_agent_chat_`：

- 验证 `bifrost-e2e` 当前可执行文件支持隐藏 worker pass-through，内置 Agent worker 能在 in-process E2E Admin 服务中正常启动。
- 验证 `/agent/chat` 非流式测试入口中的 `/stop` 返回 200 + stopped 语义，原 active chat 收敛后 session 可继续使用。
- 验证 `/reset` 删除持久化 built-in Agent history，模拟服务重启后 fresh chat 不携带 reset 前消息。

CI shell E2E worker 隔离回归：

- `test_agent_send_msg_feishu_card.sh`：worker 进程通过 `send_msg` 工具写入 Feishu card 后，主进程 message log API 重新加载磁盘 store 并能立刻查到 outbound 记录。
- `test_agent_send_msg_default_channel.sh`：worker 使用默认消息通道发送消息后，主进程 message log / schedule 相关 store 不因进程内缓存缺失而返回空数据。
- `test_agent_chat_history_continue.sh`：worker 续聊恢复 JSONL runtime state，`plan_steps` 不丢失。
- `test_agent_direct_path_switch.sh`：worker 返回 `work_dir_switched` 后主进程 session state 立即更新，后续 `/status` 显示新工作目录。
- `test_agent_run_timeline_channel_unification.sh`：worker 写入 run_state 时保留请求来源 `api` / `web`，不把所有状态归因成 `worker` 或 `admin-api`。
- `test_im_guide_queue_human_api.sh`：内置 IM Agent active turn 阻塞在 worker/mock model 时，busy 普通 IM 消息默认进入 guide；guide notify 必须唤醒主进程并立即转发给 worker，不等待 worker 产生下一条 event。

### 真实场景测试

`human_tests/agent-loop-process-isolation.md`：

- TC-ALPI-01：内置 Web/Admin Agent 请求启动后出现独立 worker，主进程继续响应代理/Admin API。
- TC-ALPI-02：`/stop` 能停止内置 worker，无需强杀主进程。
- TC-ALPI-03：SSE 客户端断开后内置 worker 被清理。
- TC-ALPI-04：外置 Runner 请求启动后出现独立 external-runner worker，主进程继续响应。
- TC-ALPI-05：`/stop` 能停止外置 Runner worker。
- TC-ALPI-06：CI/E2E runner 进程作为 `current_exe()` 时可启动 worker，`/agent/chat` 的 `/stop` 和 `/reset` 控制语义保持 200 成功响应并清理持久化历史。
- TC-ALPI-07：worker 独立进程写入 send_msg/schedule/history/timeline/work_dir 后，主进程 API 读取到最新落盘状态，覆盖 CI shell E2E 失败路径。
- TC-ALPI-08：`exec_command` 启动嵌套 shell/后台子进程后，停止 session 会清理整个 process group，不留下孤儿 `zsh/python/sleep`。
- TC-ALPI-09：模拟内置 Agent worker 启动失败时，请求返回明确错误并释放 session，不在主进程内回退执行。

### Coverage 与项目校验

- `cargo fmt --all -- --check`
- `cargo check -p bifrost-admin -p bifrost-cli`
- `cargo test -p bifrost-admin agent_worker --lib`
- `cargo test -p bifrost-admin external_cli --lib`
- `bash e2e-tests/tests/test_agent_worker_process_isolation.sh`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `cargo build --all-targets --all-features`
- 收尾执行 rust-project-validate 要求的验证矩阵。

本机若维持 no-local-coverage 约定，可跳过 `make coverage` / `make coverage-unit`；交付时说明依赖远端 CI。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：默认每个 Agent/Runner 会话独立进程；覆盖 built-in、IM、ChatGPT/Codex/custom runner；主进程只代理输入输出；stop 不依赖同进程 cooperative loop；生产 fail closed；测试仍可 in-process。
- 执行 `git status --short --branch`、`git diff`、必要 `git diff --cached`。
- Review 范围：`agent_worker.rs`、`external_cli/mod.rs`、Admin/IM Agent handlers、runner-call、stop 聚合、CLI 隐藏子命令、E2E/human_tests。
- 风险点：stdout 协议被日志污染；worker 进程组未隔离；session busy 未释放；外置 Runner stop 只停 orchestration 不停子 CLI；guide/queue 丢失或因主进程未监听 guide notify 而延迟到 worker event 后才转发；历史恢复不一致；`bifrost-e2e` 未提供 worker pass-through 导致 in-process E2E 服务把 worker 子进程误启动为普通测试 runner。
- 复测命令：`cargo fmt --all -- --check`、`cargo check -p bifrost-admin -p bifrost-cli`、`cargo test -p bifrost-admin agent_worker --lib`、`cargo test -p bifrost-admin external_cli --lib`、E2E、human_tests。

### 第 2 轮

- 复查第 1 轮修复后的最新 diff、未跟踪文件和 staged 状态。
- 检查文档、`human_tests/readme.md` 索引、E2E 脚本、CLI 隐藏命令、stop 路径和用户目标一致。
- 复跑受影响测试与 workspace 兜底：targeted cargo、E2E、`cargo test --workspace --all-features`、clippy/build。
- 若发现 worker 启动失败仍走静默 fallback，或 guide notify 仍延迟到下一次 event，立即修复并追加第 3 轮。

## 风险与决策

- Worker 进程数量随 concurrent session 线性增长；单机高并发场景（例如 Schedule 批量触发）需要 `AgentWorkerConcurrencyLimit` 或队列，避免创建上千个 worker 拖垮系统。第一版不新增队列，只在 `AgentSessionManager` 层依赖既有 busy gate；后续若观察到问题再引入。
- IPC 使用 stdio NDJSON 简化；单条 event 长度受 `AGENT_WORKER_MAX_LINE_BYTES`（默认 8 MiB）限制，超长内容通过磁盘引用（history_path / message log）传递。
- `current_exe()` 在 `bifrost-e2e` in-process 场景可能指向测试 runner；必须提供 hidden pass-through；否则 CI 里 worker 子进程会被 argparse 当作普通测试执行。
- Windows 上 process group / signal 语义不同，需要 `CREATE_NEW_PROCESS_GROUP` + `TerminateProcess` 组合；测试覆盖需要 Windows CI 单独跑 `test_agent_worker_process_isolation.sh` 的等价 pwsh 版本。
- Worker 与主进程各自持有 `ImGatewayService`：如果未来引入共享 in-memory queue，必须走 IPC 或磁盘持久化；不能假设内存单副本。
- 生产 fail closed 行为改变了旧 “worker 失败静默 fallback” 语义；升级发布时必须在 release notes 里明确“worker 启动失败会返回错误，需要检查 `bifrost` 二进制完整性”。
