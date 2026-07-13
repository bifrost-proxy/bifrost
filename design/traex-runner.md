# Trae CLI Runner Streaming 技术方案

## 背景

Bifrost 的 Agent Runner 已经把 Codex、ChatGPT Web、custom CLI 收敛到同一套 runner/adapter 模型。Trae CLI (`traecli` / `traex`) 与 Codex CLI 类似,都可以通过命令行以 JSONL 形式输出执行过程和最终结果,因此应作为新的内置 external CLI adapter 接入,而不是为 Web Chat 或飞书 IM 增加专用分支。

本次目标不是只让 Trae 跑完并返回最终答案,而是让 Trae 的执行过程进入 Bifrost 既有 timeline: Web Chat 中应有同样的运行态、过程块、工具/状态步骤和最终消息;飞书 IM 通道应继续复用 progress card 展示运行过程和最终结果。

## 用户目标验证清单

### 必须实现

- 新增内置 adapter `traex`,默认命令为 `traex --cd <work_dir> exec --json --output-last-message <last_message.md> -`。
- 支持按持久化 `threadId` 续接: `traex --cd <work_dir> exec resume --json --output-last-message <last_message.md> <thread_id> -`。
- 支持 Trae 常用执行参数: `model`、`profile`、`permissionMode`、`sandbox`、`dangerFullAccess`、`skipGitRepoCheck`、`ignoreUserConfig`、`ignoreRules`、`addDirs`、`configOverrides`、feature enable/disable、`outputSchema`、`color`、`ephemeral`。
- 支持 Bifrost 层 Codex/Traex slash 命令: `/models` 通过当前 runner executable 执行 `debug models` 并白名单展示模型;`/model [slug]` 查看或设置当前 session 的模型 override;`/model clear` 清除 override。
- External CLI runtime 必须逐行读取 stdout/stderr,解析到 JSONL progress event 后立即向调用方推送,不能等 CLI 进程结束后才批量解析。
- Web Chat `/chat/stream` 与 slash runner-call stream 必须把 Trae progress event 实时写入同一份 conversation timeline。
- IM event loop 必须把 Trae progress event 同时推给 progress card 和 conversation timeline。
- WebUI Runners 配置面支持选择 Trae CLI,并在最终消息中默认展开已完成的过程块。
- WebUI Runners 配置面只展示当前可用的 Codex CLI、Trae CLI、ChatGPT Web 三类 Adapter;`custom`/`mock` 作为内部兼容和未来扩展能力保留在协议层,不暴露给用户选择。

### 必须不破坏

- 不改变 Codex、ChatGPT Web、custom CLI 的 runner 选择语义、work_dir 继承和持久化会话恢复。
- Codex/Trae 默认作为无人值守 external runner 启动,必须默认 full access,避免 CLI 在 IM/Web 长任务中等待二次授权;用户显式配置 sandbox/permission/approval 时才按显式配置收窄。
- 不把外层 `traex` wrapper 调用作为用户可见工具步骤写进 timeline;用户应该看到 Trae 自己输出的状态、工具和最终回答。
- 不伪造 Trae 没有输出的细粒度工具事件。若 Trae 本次 JSONL 只提供状态和最终消息,Bifrost 只展示真实状态和最终消息;若 Trae 输出 tool started/finished,Bifrost 再展示工具步骤。
- 不向 Trae exec 传递不支持的 `--permission-mode default`。配置为空或 `default` 时必须默认启用 full access;当前 Trae CLI 不允许同时设置 sandbox override 和 permission mode override,因此 full access 模式只传 `--dangerously-bypass-approvals-and-sandbox`,不再同时传 `--permission-mode bypass_permissions`。
- `/models` 不能透出 Traex raw catalog 中的 `base_instructions` 等大字段或内部字段;只展示 slug/display name/description/reasoning/tier/visibility 等白名单字段,并过滤 hidden model。
- `/model` 只影响当前 `sessionKey + adapter + runnerId`,不能修改全局 runner 配置;session slash override 会覆盖 runner 默认模型,并在 Web UI、API 和 IM event loop 的下一轮 run 中统一生效。
- 未显式配置 `timeoutSecs` 时,不对 Trae/external runner 设置固定超时;长任务由用户显式 `/stop` 或 runner 自然结束控制。
- 飞书 progress card 展示工具输入/输出预览,完整 stdout/stderr 与 normalized events 保存在 run artifacts;重复的 running tool start 事件必须去重,避免卡片体过大导致中间刷新丢失。
- 飞书绑定 Codex/Trae external runner 时,如果 IM channel 没有显式配置 `deliveryMode`,必须默认使用 progress card;runner 级默认 `final_reply` 只作为非飞书或显式 channel 继承外的普通回退,不能让飞书过程卡片被静默短路。
- 不影响飞书 IM progress card 的最终收敛和普通 final reply 投递策略。

### 必须真实验证

- 使用真实 Trae CLI 执行一个 Web Chat turn,确认最终历史页显示 `Runner: traex`、过程块默认展开、最终答案可见。
- 使用真实飞书 IM 入站或已保存的飞书 session history 验证 Trae runner 输出进入同一条 timeline,并且 Web Chat 历史页能看到过程和最终结果。
- 验证 Trae `permissionMode` 空/default 配置不会生成 `--permission-mode default`,并会映射为 headless full access,避免 exec 模式报错或等待交互式授权。
- 验证没有外层 wrapper 工具调用噪音,timeline 中只保留 Trae JSONL 归一化后的事件。

## 产品语义

Trae 与 Codex 都是 "single-shot exec 模式的外部 coding agent",Bifrost 将其收敛为一个内置 adapter 家族:

- Adapter kind 层: `AdapterKind::Traex` / `AdapterKind::Codex` / `AdapterKind::ChatGptWeb`。
- Command spec 层: 根据 kind 决定 executable、subcommand、参数拼装顺序。
- Runtime 层: 复用 `ExternalCliRuntime`,统一 stdout/stderr 逐行解析,解析结果通过 channel/worker event 推给上层。
- Timeline 层: 复用 `record_external_cli_progress_event_to_timeline`,把 JSONL 事件规范成 conversation timeline 记录。

Trae 与 Codex 的关键差异:

- Trae 使用 `permissionMode`,值域 `default | plan | auto | custom | bypass_permissions`,不支持 `default` 显式传参。
- Trae full access flag 是 `--dangerously-bypass-approvals-and-sandbox`,与 sandbox 互斥;Codex 使用同名 flag 但另有 `--sandbox` / `--approval-policy` 显式收窄参数。
- Trae 支持 `debug models` 输出 catalog,`/models` 白名单化后展示。

## 技术细节

### Adapter 命令构造

`crates/bifrost-admin/src/im_gateway/external_cli/command_spec.rs` 新增 `traex` adapter 分支。默认 args 为空时,运行时生成 Trae exec/resume 命令,并把 `--cd` 放在 `exec` 前面,确保 Trae 以目标工程目录启动。`--output-last-message` 继续使用 External CLI runtime 已有的 final response 文件契约。

Codex 与 Trae 的默认配置都面向无人值守 runner: 未显式设置 `dangerFullAccess`、`sandbox` 或 Codex `approvalPolicy` 时,Codex 默认追加 `--dangerously-bypass-approvals-and-sandbox`;显式设置 sandbox/approval 时保留收窄配置。Trae 未显式设置 permission mode 时默认追加 `--dangerously-bypass-approvals-and-sandbox`,并且不同时生成 `--permission-mode`,避免 Trae CLI 报 `sandbox_mode` 与 `permission_mode` override 冲突。

Trae 的 `permissionMode` 与 Codex 的 approval policy 不同,因此单独映射为 `--permission-mode`。WebUI 把 "Headless default" 保存为空值;后端把空字符串和历史 `default` 都视为 headless full access,只输出 `--dangerously-bypass-approvals-and-sandbox`。用户显式选择 `plan`、`auto` 或 `custom` 时保留该选择,不默认启用 full access;显式设置 `dangerFullAccess` 可覆盖该行为,且 full access 优先级高于 permission mode,避免生成互斥 CLI 参数。

关键辅助函数:

- `effective_traex_permission_mode(config)`: 空字符串或 "default" 视为 None。
- `effective_traex_danger_full_access(config, permission_mode)`: `permission_mode == Some("bypass_permissions")` 或显式 `dangerFullAccess=true` 时为 true。
- `ensure_traex_work_dir_arg`: 保证 `--cd <work_dir>` 出现且位于 `exec` 前。
- `append_traex_config_args`: 逐项追加 model/profile/features/etc,重复项按 `remove_overridden_traex_args` 清理。

### 实时输出

External CLI runtime 的旧路径会等子进程结束后再解析 stdout。新增 `run_with_progress()` 后,stdout/stderr 读任务逐行捕获输出;stdout 行会先尝试解析为 `ExternalCliProgressEvent`,解析成功即通过 channel 发出,同时仍写入 run artifact。worker 模式也新增 `ExternalCliWorkerEvent::Progress`,让主进程能在 worker 仍运行时收到事件。

这条路径保证 Web Chat、runner-call、IM event loop 都能以同一套 API 获取实时 progress,而不是分别重放 artifact。

### Timeline 与展示

Web Chat 和 IM event loop 使用 `record_external_cli_progress_event_to_timeline()` 将 external CLI event 转为 conversation recorder 事件:

- `RunStarted` / `RunFinished` / `RunFailed` 写入 run state。
- `Status` 与 `AssistantDelta` 写入 assistant delta。
- `ToolStarted` 写入 tool call。
- `ToolFinished` 写入 tool call + tool result,优先使用 Trae 原始事件中的 call id、tool name、arguments。
- `AssistantFinal` 在 runner 仍运行时写入过程 timeline,作为 Trae/Codex 公开的模型 content 展示;底部最终回答仍由 run result/turn finish 统一记录为 assistant message。

前端 `ProcessStepsBlock` 运行中默认展开,完成后默认折叠。运行中按实时 conversation timeline 从上到下展示模型公开 content 和工具调用;模型公开 content 直接展示原文,不额外添加 `1.` / `2.` 这类序号。工具行默认只展示可读命令标题,点击后展开输入/输出详情。`run_state_changed` 仍是后端判定 running/completed 的内部事实源,但不渲染成 `Run state: Running` 这类用户可见过程项;UI 顶部状态标签和 thread summary 才负责表达整体状态。飞书 progress card 会把连续工具调用折叠成“已执行 N 个步骤”的一级分组,展开后再显示单条工具详情折叠项。噪音状态(run id、turn started/completed、model rerouted)不进入过程列表,避免卡片顶部被内部事件淹没。

Codex app-server 的 `fileChange` 完成事件必须从 `params.item.changes[]` 提取文件路径、`kind.type` 和 diff；展开工具详情按文件展示新增、删除和修改行数，并保留 diff 作为核验依据。app-server 对新增或删除文件可能只返回不带 `+` / `-` 的正文，此时按 `kind.type` 和正文逻辑行数兜底统计。卡片使用 Runner 的真实 `work_dir` 把工作区内绝对路径显示为相对路径，多行详情逐行缩进；artifact 仍保留原始事件。工具标题显示为“文件变更”，执行过程按“已执行 N 个步骤”计数，避免把文件编辑误称为命令。只有事件本身确实没有结构化内容时才显示“暂无工具详情”，不能因为 `result` 字段为空而丢弃 `changes[]`。

Web timeline 会按 `call_id` 合并工具 start/result,并跳过重复 start。后端在写 conversation timeline 时也会跳过同一 `call_id` 的重复 `ToolStarted`,避免 Trae/Codex 重复输出 `item.started` 时造成 WebView active command 计数虚高。

历史回放时,external runner 的 pending thinking/tool process steps 必须挂在同一轮最终 `assistant_message` 上。不能在最终回复前先 flush 成 `Agent is running...` 占位消息,否则用户在 Web UI 展开最终结果时会看不到过程信息,且时间戳/折叠状态会误导为两条 assistant 回复。

运行中的 Web Chat 不使用前端定时轮询作为状态源。后端每次向 conversation timeline 写入外部 runner 事件后,通过已有 `sessions/events` SSE 推送轻量 `timeline_changed`,payload 只包含 `sessionKey`、`historyPath` 和可用的 `endIndex`。前端只接受当前打开的 `historyPath/sessionKey` 对应事件,多个线程同时运行时不会互相写入消息区;收到事件后按本地 `endIndex` 调用 history `since` 增量接口补齐。EventSource 连接不跟随普通 thread summary 刷新重建,而是通过 ref 读取当前线程和运行状态,避免多线程同时推送时发生连接抖动或旧响应覆盖。只有 SSE lagged、重连或返回的 `start_index` 与本地 `endIndex` 不连续时,才触发一次 tail/history 或 sessions/all 校准,避免运行页把 `/sessions/all` 变成高频心跳并拖高主进程 CPU。

外部 runner (Codex/Trae) 使用 app-server transport 时支持 `turn/steer`: 同 session 的普通 busy 文本默认请求 Guide,Web Chat 展示 Guide/Queue 且默认选中 Guide;只有 `/q` 或 UI 选择 Queue 才直接进入 `SessionQueueManager`。不支持 live guide、runner 拒绝、控制通道失败或图片输入时必须明确降级排队并保留原消息/附件;成功 steer 的消息不得重复排队。`/stop` 仍作为单独控制命令立即尝试停止当前外部进程。当前 run 结束后,IM/Web Chat runner loop 只弹出显式排队或降级排队的消息启动下一轮,Codex 和 Trae 都复用上一轮保存的 `threadId` 续接 runner 原生会话上下文。ChatGPT Web 保持只支持 Queue。


Runners 配置页的 Adapter 下拉只展示产品化入口: Codex CLI、Trae CLI、ChatGPT Web。后端仍接受历史或测试用途的 `custom`/`mock` adapter,保证已有配置和自动化测试不被破坏,但新建/编辑弹窗不再把这些未来扩展项暴露给普通用户。

### Codex/Traex Slash Model 命令

Bifrost 使用 `traecli exec --json ... -` 的一轮一进程模式,不依赖 Traex TUI 的交互 slash popup。模型切换在 Bifrost 层解析:

- `/models`: 调用当前 runner 配置的 executable 执行 `debug models`,解析 JSON 后仅返回 `slug`、`displayName`、`description`、`defaultReasoningLevel`、`supportedReasoningLevels`、`visibility`、`supportedInApi`、`additionalSpeedTiers`、`serviceTiers`、`priority`。隐藏模型和 raw catalog 内部字段不进入响应。
- `/model`: 展示当前 session override;没有 override 时展示 runner/user config 解析出的模型,仍为空则说明使用 Traex 默认模型。
- `/model <slug>`: 先用 `debug models` catalog 校验 slug;不存在时返回可见拒绝消息且不写入 override。存在时把 `<slug>` 写入 `session_state.json` 的 `modelOverride`,来源为 `session slash command`。后续同 session 同 runner 的 Codex/Traex run 在 Web Chat、runner-call 和 IM event loop 中合并为 `adapterConfig.model`,最终由 command spec 追加 `--model <slug>`,包括 `exec resume`。
- `/model clear`: 删除 session override,让下一轮回到 runner 配置或 Traex 默认模型。

Web UI 在当前 runner adapter 为 `codex` 或 `traex` 时展示 `/models` 和 `/model`。`/models` 在补全菜单中回车会直接发送;`/model` 回车或 Tab 会补齐命令并把光标放到末尾,方便继续输入模型 slug。IM 通道空闲时支持同一命令,运行中发送 `/model` 只提示等待当前任务结束,避免把控制命令当普通 prompt 送入 Codex/Traex。`/efforts` 与 `/effort` 在 busy 状态也必须由 Bifrost 命令层优先处理；设置只影响下一轮，不能经默认 Guide 进入当前 `turn/steer`。Slash 命令结果作为 display-only system message 写入 `session_state.json` 供刷新回放;即使该会话已有 canonical JSONL timeline,session detail 也必须把这些 system display messages 合并到返回的 `messages` 中,并保持为独立居中系统行。该消息不注入 runner prompt,避免污染模型上下文。

Agent Chat 底部 token HUD 需要从 session detail、history summary 和外部 runner metadata 合并 `model`、`modelProvider`、`usageTotalTokens`、`usageInputTokens`。刷新页面、加载 history、发送下一轮消息以及运行中的 status 空快照都不能把已知模型、token 和 context 覆盖为空或 0。

## CLI + Web + Admin API

| 入口 | 命令/路径 | 用途 |
| --- | --- | --- |
| CLI | `traex exec --json --output-last-message <path> -` | Bifrost 通过命令行执行 Trae 单轮任务 |
| CLI | `traex exec resume --json --output-last-message <path> <thread_id> -` | 续接持久化 thread |
| CLI | `traex debug models` | `/models` 命令后端调用来源 |
| Web UI | Agent Chat `ProcessStepsBlock` | Trae 过程/结果展示 |
| Web UI | `/models`, `/model [slug]`, `/model clear` slash | session 模型 override |
| Admin | `/chat/stream` | Web Chat 走的实时流 |
| Admin | `/api/im_gateway/*` | 飞书 IM event loop |
| Admin | `/api/sessions/events` (SSE) | `timeline_changed` 增量推送 |
| Admin | `/api/sessions/{sessionKey}/history?since=<endIndex>` | 增量拉取 timeline |

## Sync 边界

- Trae runner 配置属于本机 Runners 配置的一部分,由 Runners 存储层持久化。目前不进入远端 Sync payload。
- Session `modelOverride` 只写入本机 `session_state.json`,不上行。
- `debug models` catalog 每次按需现拉,不做跨设备缓存。

## Phase 1: Adapter 命令与 runtime 集成

- 在 `AdapterKind` 增加 `Traex` 变体与 command spec。
- 实现 `effective_traex_permission_mode` / `effective_traex_danger_full_access` / `ensure_traex_work_dir_arg` / `append_traex_config_args` / `remove_overridden_traex_args`。
- `ExternalCliRuntime::run_with_progress` 提供实时 stdout/stderr 逐行解析。

## Phase 2: Web Chat / IM 通路

- `record_external_cli_progress_event_to_timeline` 覆盖 Trae/Codex 事件语义。
- `/chat/stream`、runner-call stream、IM event loop 全部走同一 timeline recorder。
- 飞书 progress card 默认投递、连续工具折叠、噪音事件过滤。

## Phase 3: Slash 命令与模型 override

- `/models`、`/model [slug]`、`/model clear` 在 Codex/Trae adapter 下解析。
- Session override 写入 `session_state.json`,应用到下一轮 command spec。
- Token HUD 与 model catalog 白名单过滤。

## Phase 4: WebUI 与真实场景

- Runners 配置 Adapter 下拉仅暴露 Codex/Traex/ChatGPT。
- Agent Chat 过程块默认展开/折叠、Threads 折叠状态 localStorage、fallback runner mark。
- 更新 human_tests、design 文档。

## 测试方案

### 单元测试

- `traex_adapter_builds_exec_command_with_prompt_stdin`: 验证默认 Trae exec 命令、参数顺序和不设置固定 timeout。
- `traex_adapter_builds_resume_command_from_thread_id`: 验证 threadId 续接命令。
- `traex_adapter_defaults_to_headless_full_access_for_exec`: 验证空 permission mode 默认映射为 headless full access,且不同时生成 `--permission-mode`。
- `traex_adapter_maps_default_permission_mode_to_headless_full_access`: 验证历史 `default` 不传给 Trae,而是启用 full access 且不同时生成 `--permission-mode`。
- `traex_adapter_respects_explicit_non_bypass_permission_mode`: 验证显式 `plan`、`auto` 或 `custom` 不被默认 full access 覆盖。
- `traex_adapter_applies_session_effort_to_command_spec`: 验证 session effort/model 合并到命令。
- `codex_adapter_defaults_to_danger_full_access_for_headless_runs`: 验证 Codex 空配置默认追加 full access 参数。
- `codex_adapter_respects_explicit_sandbox_without_danger_full_access`: 验证显式 sandbox 不被默认 full access 覆盖。
- `traex_cli_parser_maps_real_jsonl_events`: 验证 Trae JSONL 事件可归一化。
- `external_cli_runtime_streams_stdout_before_process_exit`: 验证 stdout 事件在进程退出前已经推送。
- `external_progress_maps_to_agent_turn_progress_events`: 验证 external progress 可转 IM progress card 事件。
- `external_runner_progress_events_are_recorded_as_visible_timeline_steps`: 验证 status/tool 事件写入可见 timeline。
- `assistant_final_is_pipeline_content_until_turn_finished`: 验证 Trae/Codex 公开 `agent_message` 在 runner 仍运行时进入过程区域,不提前占用底部最终结论。
- `timed_out_external_cli_result_reports_failure_reply`: 验证 Trae 超时等非成功状态按失败收敛,不把早期 `agent_message` 当作成功结果。
- `duplicate_running_tool_started_updates_existing_pipeline_item`: 验证重复 `item.started` 不重复插入工具过程,且工具详情输出预览限长。
- `external_runner_duplicate_tool_started_is_recorded_once`: 验证后端 timeline 持久化不会重复写入同一 `call_id` 的工具 start。
- `historyEventsToMessages deduplicates repeated external runner tool events by call id`: 验证 Web timeline 按 `call_id` 合并重复 start 与 result,保留输入输出。
- `feishu_codex_like_external_runner_defaults_to_progress_card_without_channel_override`: 验证飞书 Codex/Trae runner 即使 runner 级配置为 `final_reply`,没有 channel delivery override 时仍使用 progress card;显式 channel/input override 仍优先。
- `historyEventsToMessages attaches external runner process steps to the final assistant message`: 验证已保存的飞书/Trae history 展开最终 assistant 时可以看到同轮 thinking/tool 过程,而不会产生单独 `Agent is running...` 占位结果。
- `traex_model_slash_command_parser_handles_list_show_set_and_clear`: 验证 Codex/Traex `/models`、`/model`、`/model <slug>`、`/model clear` 解析和非法 slug 拒绝。
- `traex_model_catalog_parser_filters_raw_catalog_to_safe_public_fields`: 验证 raw catalog 只输出白名单字段,过滤 hidden model,不泄露 `base_instructions`。
- `apply_persisted_state_applies_codex_and_traex_session_model_override`: 验证 session override 合并到 Codex/Traex request,并覆盖 runner 默认模型。

单测集中在:
- `crates/bifrost-admin/src/im_gateway/external_cli/tests.rs`
- `crates/bifrost-admin/src/im_gateway/progress_card/tests.rs`
- `crates/bifrost-admin/src/handlers/im_gateway/tests.rs`
- `web/src/pages/AI/AgentChatSection.timeline.test.ts`

### E2E 测试

- `e2e-tests/tests/test_im_gateway_traex_runner_streaming.sh`: 端到端跑 Trae runner,断言实时 stdout 与 progress card。
- `e2e-tests/tests/test_im_gateway_traex_model_slash.sh`: 覆盖 `/models`、`/model <slug>`、`/model clear`。
- `e2e-tests/tests/test_im_gateway_external_runner_delayed_final_state.sh`: 覆盖延迟 final state 的收敛。
- `e2e-tests/tests/test_im_gateway_external_runner_image_input.sh`: 覆盖图像输入的 progress 语义。
- 使用临时 `BIFROST_DATA_DIR`、非 9900 端口,启动服务后调用 `/chat/stream`,断言 NDJSON 中包含 Trae progress event、最终 `run_finished`、run detail artifacts 和 timeline。
- Playwright 断言 external runner 运行中输入只显示 Queue 并发送 `/q ...`;断言 Threads 折叠状态写入 localStorage,刷新后仍保持;断言 Trae fallback thread mark 不显示为 `Bf`;断言运行中 history 由 `timeline_changed` SSE 触发增量更新,其他线程的 timeline 事件不会污染当前消息区,且不会高频请求 `/sessions/all`。
- Playwright 打开 Agent Runners 的 Add Runner 弹窗,断言 Adapter 下拉包含 Codex CLI、Trae CLI、ChatGPT Web,且不包含 Custom、Mock。
- 使用 mock `codex` 和 mock `traecli` 启动真实 Bifrost,调用 `/chat/stream` 发送 `/models`、`/model Doubao-Unit` 和普通消息,断言模型列表不泄露 raw 字段,非法模型被拒绝且不写入 override,后续 run snapshot 带目标 `--model`,session state 写入 override。

### 真实场景测试

- 更新 `human_tests/im-gateway-external-cli-chat-gateway.md`,新增 Trae Web Chat、飞书 IM progress card、permission mode 默认 headless full access 三个用例。
- 对本轮回归新增飞书 Codex/Trae progress card 默认投递和 Web history 最终回复过程挂载用例。
- 按用例真实执行: 临时端口、临时数据目录、`--no-system-proxy`、禁用 Sync 自动登录弹窗、`BIFROST_DISABLE_TRAY=1`。
- WebUI 亮色/暗色至少验证 Agent Chat 过程块可读性;本轮主要变更不新增硬编码主题色。
- 更新 `human_tests/im-gateway-external-cli-chat-gateway.md` 的 TC-IEC-50,覆盖 Traex `/models` 与 `/model` session 模型切换,并按真实临时服务/API/Web UI 链路执行。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标: Trae runner、实时输出、Web Chat 体验、飞书 IM progress card 体验、最终过程可见。
- Review 范围: `external_cli` runtime/command spec/parser、Chat Gateway stream、IM event loop、Agent Chat 前端、Runners 配置 UI、design/human_tests。
- 复测命令: focused Rust 单测、真实 WebUI Trae run、human_tests 新增用例。

### 第 2 轮

- 复查第 1 轮修复后的 diff,重点检查是否仍有 wrapper tool 噪音、final assistant 重复、Trae permission mode 错误、ChatGPT Web/Codex 回归。
- 复测命令: focused Rust 单测、`cargo test --workspace --all-features`、clippy/fmt、必要时 local-ci。

## 校验要求

- 先执行本模块相关 E2E 和 human_tests,再执行 rust-project-validate。
- 提交前至少执行一次 `cargo test --workspace --all-features`。
- WebUI 变更需要通过前端构建或由 `cargo run --bin bifrost -- start` 的前端构建阶段验证。
- 本地约定 no-local-coverage,不跑 `make coverage`。

## 风险与决策

- 决策: Trae 使用同一 `ExternalCliRuntime`。原因: 避免为每种 external agent 再造 event pipeline,共享 tool dedup / assistant final 归属 / timeline_changed SSE 等能力。
- 决策: Session `modelOverride` 只保存在 `session_state.json`。原因: 与 Runner 全局配置区分,避免 slash 命令误改产品化配置;跨设备不同步,减少行为漂移。
- 风险: Trae CLI 未来可能改变 JSONL 事件 schema。缓解: 解析层 (`traex_cli_parser`) 覆盖真实 JSONL 样本,并对未知事件类型退化为纯 stdout artifact,不阻断 run。
- 风险: `debug models` 输出中 `base_instructions` 等字段体量大且含敏感 prompt。缓解: catalog parser 白名单只保留公开字段。
- 风险: 飞书 progress card 事件量大时中间刷新可能失败。缓解: 连续工具事件按 call_id dedup 并折叠,`progress_card` 只保留 head/tail 摘要,完整内容仍在 run artifact。
- 风险: 前端 `sessions/events` SSE 在 lagged 时可能补齐延迟。缓解: 仅在 `start_index` 与本地 `endIndex` 不连续时才 fallback 到 `/sessions/all`,常规运行不高频轮询。
- 风险: 用户显式配置 sandbox + permission-mode 时,Trae CLI 会拒绝。缓解: `effective_traex_danger_full_access` 与 `effective_traex_permission_mode` 二择一输出,`remove_overridden_traex_args` 清理重复项。
