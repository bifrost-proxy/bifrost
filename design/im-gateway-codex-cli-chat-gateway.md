# Agent Custom Runner / Codex CLI Chat Gateway 设计方案

## 背景

IM Gateway 接入 Codex CLI 后，真实 IM 入站仍然是最终验收目标，但开发、E2E 和回归验证不能依赖人工从飞书或微信发消息。需要一个 Chat Gateway，让开发者和测试脚本通过 HTTP/API 发起同等语义的消息，复用 IM Gateway 的清洗、会话调度、外部 CLI Agent runtime、进度事件监听和 IM 风格渲染逻辑。Codex 是首个目标 CLI，但架构必须支持后续平滑接入 Claude Code、Gemini CLI、Trae CLI、Cursor Agent 或内部 CLI Agent。

Chat Gateway 不是新的 Agent 产品形态，也不是绕过 IM Gateway 的快捷入口。它是 IM Gateway 的 provider-neutral 测试/调试入口：把“IM 入站事件”替换成“可构造的 Chat 请求”，其余执行链路保持一致。

## 用户目标验证清单

### 必须实现

- 提供 HTTP Chat Gateway，可直接调用接口触发外部 CLI Agent 执行，Codex CLI 是首个内置 adapter。
- Chat Gateway 请求经过与 IM 入站一致的消息清洗、session key 解析、队列/busy 处理和 progress event 渲染。
- 支持同步返回最终结果，也支持流式读取进度事件，方便自动化测试断言。
- 支持注入工程目录、route/provider/global 指令、skill roots、Bifrost 工具集合和 adapter-specific CLI 执行参数。
- 支持构造 IM 上下文元数据（provider、chat_id、user_id、message_id、reply target），用于验证 send/update 消息类工具。
- 每次 run 生成不可变 `runtime_snapshot.json`，落盘 stdout/stderr、normalized events、last_message，便于审计和复现。
- 已内置 Codex 与 Claude-Code 两个 adapter；`defaultRunnerId` 默认 `Codex`。

### 必须不破坏

- 不改变现有 Feishu/Weixin 长连接入站处理。
- 不绕过 Remote Invoke / IM Gateway 已有权限边界；测试入口显式标记为本地/admin 调试能力。
- 不把测试请求中的任意 work_dir 直接放开到外部 CLI Agent；必须经过 allowlist 或配置解析。
- 不让 Chat Gateway 和 IM Gateway 各自实现两套 message sanitizer、progress renderer 或 session queue。
- 不把 secret、provider token、完整 provider config 暴露到 Chat Gateway 响应。
- 已有内置 Bifrost Agent runtime 继续可选；`AgentConfig.runner=bifrost_agent` 保持默认可选值。

### 必须真实验证

- 通过真实 HTTP 请求触发外部 CLI Agent runtime，不依赖人工 IM 消息。
- Codex CLI JSONL 输出被 adapter 转换为 Bifrost `AgentTurnProgressEvent`；后续其他 CLI 输出也进入同一 canonical event 模型。
- 断言最终消息与流式事件都可被测试脚本读取。
- 断言模拟 IM 上下文下，send/update 消息工具使用受控目标而不是泄漏到真实 IM。
- Codex/Trae 图片入站：图片被下载并落到 session 附件目录，prompt 中出现 `## Attached Images` 与绝对路径。

## 产品语义

### 一个 runtime、两个入口

- 真实 IM 入站与 HTTP Chat Gateway 使用同一个 pipeline：`Provider Event Normalizer` / `Chat Request Normalizer` → `MessageSanitizer` → `SessionQueueManager` → `AgentRuntime`（`BifrostAgent` 或 `ExternalCliAgent`）→ `AgentTurnProgressEvent` → `Progress Renderer / Test Stream`。
- Chat Gateway 只是构造入站上下文的 `NormalizedInboundMessage`，字段包含 `source_kind` (`im`/`chat_gateway`)、`provider_id`、`provider_type`、`chat_id`、`user_id`、`message_id`、`text`、`images`、`reply_mode` (`none`/`test_stream`/`real_im`)、`target`。
- 后续 session key、busy 队列、`/status`、`/stop`、`/clear`、progress card snapshot 都不区分消息来源。

### 测试流与真实 IM 渲染解耦

- 真实 IM 渲染走 provider `send_text` / `send_card` / `patch_card`。
- Chat Gateway 默认把同一组 progress event 暴露给调用方：
  - 同步模式：阻塞到 turn 结束，返回 final response、工具日志、进度快照。
  - 流式模式：NDJSON 输出 `AgentTurnProgressEvent`。
  - 回放模式：给定 `run_id` 读取历史 JSONL / event log。
- 只有 `reply_mode=real_im` 且调用方具备 admin 权限时，才把结果发送到真实 provider/target。

### Runtime adapter 插件化

Codex CLI 只是第一个 `ExternalCliAgentAdapter`，后续新增 adapter 不改 IM event loop、session queue、progress renderer 和 Chat Gateway API。核心抽象：

```rust
trait AgentRuntime {
    fn runtime_id(&self) -> &'static str;
    async fn run_turn(&self, input: AgentRunInput, sink: AgentProgressSink) -> Result<AgentRunResult>;
    async fn stop(&self, run_id: &str) -> Result<()>;
}

trait ExternalCliAgentAdapter {
    fn adapter_id(&self) -> &'static str;
    fn build_command(&self, snapshot: &AgentRuntimeSnapshot) -> Result<CommandSpec>;
    fn build_prompt(&self, snapshot: &AgentRuntimeSnapshot, input: &AgentRunInput) -> Result<String>;
    fn parse_stdout_line(&self, line: &str) -> CliEventParseResult;
    fn final_response(&self, run_dir: &Path, parsed: &ParsedCliRun) -> Result<String>;
}
```

`ExternalCliAgentRuntime` 是通用进程托管 runtime，负责 run dir、进程生命周期、stdout/stderr、超时、stop、artifact、事件落盘。adapter 只负责 CLI 差异：命令参数、prompt envelope、stdout 事件解析、最终答案提取。

### Canonical progress event

所有 runtime 归一化为 Bifrost canonical progress event：`RunStarted` / `StatusChanged` / `AssistantDelta` / `AssistantFinal` / `PlanUpdated` / `ToolStarted` / `ToolFinished` / `ArtifactCreated` / `RunFinished` / `RunFailed` / `RunStopped`。adapter 允许输出能力缺失，UI 侧渲染为对应模块空。

Codex 归一化：`command_execution` → `ToolStarted`/`ToolFinished`，tool name `exec_command`，arguments 用 `item.command`，result 用 `item.aggregated_output`，success 用 `item.exit_code == 0`，call id 用 `item.id`。Claude Code stream-json 的 `tool_use` 与 `tool_result` 按 `tool_use_id` 成对归一化，Bash 工具优先展示 `input.command` 与 `tool_use_result.stdout/stderr`。

### 完成状态权威来源

外部 CLI stdout JSONL 是过程流，不等同于最终可见答案已持久化。顺序必须遵守：

1. `RunStarted` → `run_state_changed: running`。
2. `AssistantDelta` / `AssistantFinal` / tool events 只写过程 timeline。
3. progress `RunFinished` 不能写 `run_state_changed: completed`。
4. `ExternalCliRunResult` 生成后，`record_external_cli_web_turn_result` 先写 `assistant_message`，再写 terminal `run_state_changed: completed/failed`。
5. `sessions/all`、session detail、Web History、IM progress card 只把第 4 步之后的 terminal run_state 当作 Ready。

回归测试必须覆盖 stdout `turn.completed` 早于最终 response 的场景。

### 外部 Runner 状态展示

外部 CLI runner 不能照搬内置 Agent 的 loop/context/compaction 指标。飞书 progress card 与 Web Chat 显示：

- 状态标题：`Runner` 和模型标签；配置了 `adapterConfig.model` 时展示真实模型名和来源。
- 状态正文：运行状态、Runner、Adapter、模型、外部会话、队列/引导、工作路径、最新工具摘要、token usage、最近输入 context（来自 `turn.completed.usage.input_tokens` 近似值）。
- 不展示内置 Agent 专属的 `Loop 0/0`、`压缩 0 次` 空指标；未知字段保持 N/A。
- 隐藏 chain-of-thought 不可见，也不伪造；只展示 CLI 明确输出的 reasoning summary / status / tool / final。

### 图片附件桥接

Feishu/Weixin 入站图片先由 provider resolver 下载为 `ChatImageInput`；命中自定义 runner 时，event loop 转换为 `ExternalCliImageInput` 放入首轮 external CLI request；排队后续文本消息不复用上一轮图片。Web Chat 走 `/chat/stream` request 的 `images[]`。runtime 在执行前把图片落盘到 `sessions/YYYY/MM/DD/attachments/<session-file-stem>/<run-id>/images/image-N.<ext>`，prompt 前追加 `## Attached Images` 列出绝对路径、mime、大小。`attachmentBaseDir` 只能由服务端基于已验证的 session recorder 生成，禁止调用方指定任意根目录。

### 能力声明与降级

adapter 声明能力，WebUI/API 按能力显隐配置项：

```json
{
  "adapter_id": "codex",
  "display_name": "Codex CLI",
  "capabilities": {
    "json_events": true, "resume": true, "images": true,
    "sandbox": true, "approval_policy": true, "mcp": true,
    "skills": true, "output_last_message_file": true
  }
}
```

不支持能力隐藏或标记“不适用”；运行时降级原因写入 `runtime_snapshot.json`。

## 技术细节

### API

- `POST /api/im-gateway/chat`：同步执行一个 turn，返回 final response、run_id、artifacts。
- `POST /api/im-gateway/chat/stream`：NDJSON 流式返回 canonical progress events。
- `GET /api/im-gateway/chat/runs/:run_id`：读取 runtime snapshot、stdout/stderr、normalized events、final response。
- `GET /api/im-gateway/chat/runs/:run_id/events`（planned，截至 2026-06-16 未落地）。
- `POST /api/im-gateway/chat/runs/:run_id/stop`：写 stop marker，向 active CLI process 发终止信号。Unix 下只在确认子进程拥有独立 process group 时才终止同组；run 收敛阶段优先识别 stop marker，即使 shell 迟到输出也固定返回 `status:"stopped"`。Windows `taskkill` 把明确的 `process not found` / `no running instance of the task` 视为幂等成功。
- `GET/PATCH /api/im-gateway/chat/config`：完整 runner registry。
- `GET/PATCH /api/im-gateway/chat/config/channels/:provider_id`：`ExternalCliChannelSettings`（`runnerId/enabled/deliveryMode`）。
- `POST /api/im-gateway/agent/resolve-config`（planned）：输入 provider/route/work_dir/request overrides 返回脱敏 `AgentRuntimeSnapshot`。

### Runtime 配置合并

层级：`Global Agent Defaults` → `IM Provider / Channel Agent Config` → `Route / Schedule / Chat Gateway Request Override` → `Single Run Runtime Snapshot`。

合并顺序：
1. 全局 `agent.external_cli` / `agent.adapters`。
2. provider/channel 的 `agent_config`。
3. route、schedule 或 Chat Gateway request override。
4. 单次 run 临时字段。

字段规则：

| 字段类型 | 合并规则 |
| --- | --- |
| 标量 (`runtime`/`adapter`/`model`/`work_dir`) | 后一级非空覆盖前一级 |
| 指令 `developer` / `user` | 默认追加，global 在前，channel/route/request 在后；`replace_*` 才替换 |
| `allowed_work_dirs` | 交集或受 global allowlist 约束的子集 |
| `add_dirs` | 并集，但每项必须通过 global allowlist |
| `skills.enabled` | 后一级可追加启用，不能启用 global policy 禁止的 skill |
| `skills.disabled` | 追加禁用，禁用优先级最高 |
| `mcp_servers` | 按 server name 合并；secret/env 来自安全存储 |
| `reply_mode=real_im` | 必须 global 允许 + channel 允许 + request 显式 + 权限校验 |

`AgentConfig.runner` 与 `ImProviderAgentConfig.runner` 使用统一 runner 语义：`bifrost_agent` 表示内置 Agent，自定义 runner 直接保存 runner ID（`Codex`、`Claude-Code`、`abc` 等）。

### Codex adapter 参数映射

`profile`→`--profile`；`profileV2`→`--profile-v2`；`model`→`--model`；`sandbox`→`--sandbox`；`dangerFullAccess`→`--dangerously-bypass-approvals-and-sandbox`（并抑制 `--sandbox`）；`reasoningEffort/reasoningSummary`→`--config model_reasoning_*="..."`；`skipGitRepoCheck`→`--skip-git-repo-check`；`ignoreUserConfig`→`--ignore-user-config`；`ignoreRules`→`--ignore-rules`；`addDirs[]`→重复 `--add-dir`；`configOverrides[]`→重复 `--config`；`enableFeatures[]`→重复 `--enable`；`disableFeatures[]`→重复 `--disable`。历史 `search:true` 兼容映射为 `--enable web_search`。Runtime 始终注入 `--cd <resolved_work_dir>` 与 `--output-last-message <run>/last_message.md`，即使 runner 自定义了 `args`。

Schedule Agent 覆盖：以 schedule 覆盖值覆盖 Runner 默认值；`dangerFullAccess=true` 时移除模板已有 `--sandbox`；`bifrost im schedule add/update` 必须同时提供 `--target` 或 `--provider/--target-mode`。

### Claude-Code adapter

默认执行 `claude -p --verbose --output-format stream-json --input-format text`。未显式 `permissionMode` 时追加 `--dangerously-skip-permissions`；`permissionMode` 映射到 Claude Code camelCase 值；`model`→`--model`；`reasoningEffort`→`--effort`；`addDirs`→`--add-dir`。`/models` 列出 `sonnet`/`opus`/`fable`；`/model <slug>` 持久化到 `sessionKey + adapter + runnerId` 的 `modelOverride`，下一条普通消息通过 `claude --model <slug>` 启动。`session_id`/`thread_id`/`threadId` 事件写入 metadata `threadId`，用于排队/schedule/session state 续接。

### Skill 与 Bifrost 工具集合

Skill resolver：`<work_dir>/.agents/skills`、`~/.agents/skills`、`~/.codex/skills`、`$BIFROST_DATA_DIR/agent/skills` 与 `.system`。prompt 中只放 name、description、绝对路径；不 eager 注入完整 SKILL.md。Codex adapter 用 `--add-dir` 让 CLI 读取 skill root；Bifrost 内置能力优先通过 `bifrost install-skill -t codex -y` 安装为 Codex 原生 skill。

V1 通过 skill 调用 `bifrost` CLI（`traffic list/search/get`、`im send`、`im provider status`、`remote ...`、`rules ...`）。Chat Gateway 测试默认不允许 `bifrost im send` 发到真实 IM。V2 新增 `bifrost mcp-server` 结构化工具：`im_send`、`im_update_progress`、`traffic_search/get`、`rule_list`、`remote_status/invoke`、`schedule_*`。

### 进程与 run dir

每个 run 独立 run dir：

```text
$BIFROST_DATA_DIR/im_gateway/chat_runs/<run_id>/
  request.json prompt.md
  cli.stdout.log cli.stderr.log
  normalized_events.jsonl last_message.md
  runtime_snapshot.json meta.json
```

监听：stdout 原样落盘再交 adapter parser，未识别行落盘不阻断；stderr 落盘并节流摘要；parser 转 canonical event；final response 优先 `last_message.md`，为空时用最后一个 assistant/final event 兜底；进度事件进入 `ImAgentProgressRegistry` 与 Chat Gateway stream。

### 会话状态持久化与默认续接

`session_state.json` 按 `sessionKey + adapter + runnerId` scope 保存 threadId 与 modelOverride，用于跨轮 resume。Codex/Traex app-server 运行中通过 `turn/steer` 接收 Guide；Claude Code 与自定义/exec transport 先请求 active worker capability，无法注入时完整降级 queue。只有 `/q` 始终显式排队，ChatGPT Web 保持默认 queue；`/stop` 映射到 active runner 进程并终止其独立进程组。

## CLI + Web + Admin API

- CLI：`bifrost im schedule add/update ... --provider <p> --runner <r> --target ...`；`bifrost install-skill -t codex -y`。
- Admin API：如上 `chat`/`chat/stream`/`runs/:id`/`runs/:id/stop`/`chat/config`/`chat/config/channels/:provider_id`；`agent/defaults`、`agent/providers/:id/agent`、`agent/resolve-config` 为 planned。
- Web：AI → Agent → Runners 管理 runner；AI → Agent → General 选默认 runner；IM Provider 编辑弹窗覆盖 `agent_config.runner`；Chat Gateway 测试抽屉 + Runs 详情页共用同一入口。真实 IM 入站和 Chat Gateway 测试 run 都进入同一 Runs 页面，按 `source_kind` 区分。

## Sync 边界

- `im_gateway_external_cli_agent.json` 保存 runner registry，属于本地 Agent 配置。
- Chat Gateway run dir、runtime_snapshot、artifact 不进 Sync。
- Remote Invoke 只能读安全 summary；`bifrost remote im ...` 不能读 secret/token/完整 provider config。
- 图片附件目录只允许服务端派生，禁止 request 指定 `attachmentBaseDir`。

## 实现切分

### Phase 1：core runtime + Chat Gateway

- 建立 `ExternalCliRuntime` 与 `ExternalCliAgentAdapter` trait；实现 Codex adapter；`POST /chat`、`GET /runs/:id`。
- run dir 结构、`runtime_snapshot.json`、canonical event 归一化。
- 单元测试：`chat_gateway_request_normalizes_im_context`、`external_cli_adapter_parser_maps_progress_events`、`codex_cli_parser_maps_real_command_execution_events`、`chat_gateway_rejects_unallowed_work_dir`。

### Phase 2：流式 + stop + IM route

- `POST /chat/stream` NDJSON。
- `POST /runs/:id/stop` + stop marker + 进程组终止 + Windows `taskkill` 幂等处理。
- `ExternalCliAgentChat` route action：IM 真实入站命中后走 external CLI runtime。
- 新增 `ProgressCard` delivery 覆盖外部 runner 状态字段。

### Phase 3：runner registry + Claude-Code

- `im_gateway_external_cli_agent.json` runner registry；`defaultRunnerId + runners{}`。
- 内置 `Codex` 与 `Claude-Code`；`AgentConfig.runner` 与 `ImProviderAgentConfig.runner` 覆盖入口。
- `/chat/config`、`/chat/config/channels/:provider_id` API + WebUI。

### Phase 4：图片桥接 + session state + WebUI 完善

- Feishu/Weixin 图片桥接 → session attachment dir；prompt `## Attached Images`。
- `session_state.json` 按 `sessionKey + adapter + runnerId` 保存 threadId/modelOverride。
- WebUI：Agent Defaults / 通道 Agent tab / Chat Gateway 测试抽屉 / Runs 详情；亮暗主题验证。
- `human_tests/im-gateway-external-cli-chat-gateway.md` + `human_tests/readme.md` 索引。

## 测试方案

### 单元测试

- `chat_gateway_request_normalizes_im_context` / `chat_gateway_rejects_unallowed_work_dir`。
- `external_cli_adapter_parser_maps_progress_events`；`codex_cli_parser_maps_real_command_execution_events`；`codex_command_execution_progress_is_recorded_as_exec_command_tool_steps`。
- `external_runner_status_footer_uses_runner_metadata_instead_of_agent_metrics`；`feishu_progress_card_expands_process_while_running_and_collapses_after_finish`。
- `codex_cli_parser_maps_reasoning_summary_to_assistant_delta`；`codex_request_metadata_includes_configured_or_default_model_label`；`codex_and_traex_metadata_include_runner_observability`；`progress_event_observation_adds_tool_duration`。
- `external_cli_adapter_capabilities_drive_config_schema`；`external_cli_runtime_accepts_manifest_adapter`。
- `chat_gateway_real_im_requires_permission`；`agent_effective_config_marks_inherited_and_overridden_fields`；`agent_effective_config_rejects_channel_work_dir_expansion`。
- `request_run_stop_treats_missing_active_pid_as_stopped`；`taskkill_missing_process_messages_are_idempotent`。
- `external_cli_images_from_chat_images_preserves_payloads`；`session_attachment_base_dir_uses_history_file_stem`；`external_cli_run_writes_image_attachments_and_injects_prompt_paths`。

### E2E 测试

- `im_gateway_external_cli_chat_gateway_basic` / `_stream` / `_skill_context` / `_stop`。
- `im_gateway_codex_adapter_contract`：`codex exec --json`、`--cd`、`--add-dir`、`--output-last-message` 契约。
- `test_im_gateway_codex_runner_streaming.sh`：真实 Codex CLI 触发 `pwd`，断言 `tool_started` 早于 `run_finished`。
- `im_gateway_agent_config_webui_flow` / `_theme`。
- `im_gateway_external_runner_image_input` / `_im_images`。

### 真实场景测试

`human_tests/im-gateway-external-cli-chat-gateway.md` 覆盖：HTTP 触发外部 CLI、Codex adapter、Codex runner 实时 `exec_command`、同步/流式响应、非 allowlist work_dir 拒绝、默认不真实回 IM、`reply_mode=real_im` + 权限、WebUI 全局/通道/预览/测试抽屉、亮暗主题、Feishu/Weixin 图片入外部 runner、WebUI 粘贴图片走外部 runner 与 slash runner-call。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-admin im_gateway::external_cli`
- 新增/更新 E2E。
- `cargo test --workspace --all-features`
- 按修改范围评估 `scripts/ci/local-ci.sh`。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核入口边界：Chat Gateway 只替代入站事件，不复制 runtime。
- Review API schema、work_dir allowlist、reply_mode 安全边界。
- 使用 mock external CLI executable 跑单元和 E2E，单独覆盖 Codex adapter contract。
- 修复 parser、stream 或权限遗漏。

### 第 2 轮

- 复查第 1 轮修复后的 diff。
- 复跑 `/chat`、`/chat/stream`、stop、skill context、work_dir 拒绝路径。
- 检查 `human_tests` 文档和 readme 索引。
- 如仍发现协议、安全或可测性缺口，追加第 3 轮。

## 风险与决策点

- **admin token vs. provider owner 身份**：Chat Gateway 默认只允许 admin token；`reply_mode=real_im` 需显式 provider/target + send permission。
- **流式协议 NDJSON vs. SSE**：当前 `/chat/stream` 采用 NDJSON，贴近 CLI JSONL 输出；WebUI 内部亦复用 NDJSON。
- **外部 CLI session 复用**：Codex/Traex app-server 的 Guide 留在当前 turn；显式 `/q` 或 Guide 失败降级后的下一轮复用 `threadId`，Claude Code 通过 metadata `threadId` 关联。
- **`/chat/stream` 实时性**：当前为 run 级 NDJSON，尚未做 stdout 行级实时转发；作为下一步增强。
- **Bifrost 工具集合形态**：V1 通过 skill + CLI；V2 补 `bifrost mcp-server`。
- **能力边界诚实**：Codex/Trae CLI 未稳定输出的 context window、剩余 context、自动压缩节省 token 等字段不得伪造，缺失显示 N/A；只展示 CLI 明确输出的 reasoning summary/status/tool/final。
- **Web Agent Chat 完成状态权威**：`sessions/all` 或 active detail 的 live summary 高于 JSONL timeline replay；detail `run_state=idle` 时旧 timeline 不能让页面显示 Running。`/q` 与 `/rq` 是队列控制命令，禁止写入 JSONL user_message 或展示为普通气泡。
- **Codex CLI 版本适配**：本机验证基于 `codex-cli 0.130.0` / `0.136.0`；已去掉旧 `--ask-for-approval` 参数。后续 Codex CLI 参数变化需在 adapter 中显式回归。
- **附件目录安全**：`attachmentBaseDir` 只允许服务端派生；request 中的 `attachmentBaseDir` / `attachment_base_dir` 必须删除或覆盖；不满足约束时降级写入 run dir 内部附件目录。
