# Trae CLI Runner Streaming 技术方案

## 背景

Bifrost 的 Agent Runner 已经把 Codex、ChatGPT Web、custom CLI 收敛到同一套 runner/adapter 模型。Trae CLI 与 Codex CLI 类似，都可以通过命令行以 JSONL 形式输出执行过程和最终结果，因此应作为新的内置 external CLI adapter 接入，而不是为 Web Chat 或飞书 IM 增加专用分支。

本次目标不是只让 Trae 跑完并返回最终答案，而是让 Trae 的执行过程进入 Bifrost 既有 timeline：Web Chat 中应有同样的运行态、过程块、工具/状态步骤和最终消息；飞书 IM 通道应继续复用 progress card 展示运行过程和最终结果。

## 用户目标验证清单

### 必须实现

- 新增内置 adapter `traex`，默认命令为 `traex --cd <work_dir> exec --json --output-last-message <last_message.md> -`。
- 支持按持久化 `threadId` 续接：`traex --cd <work_dir> exec resume --json --output-last-message <last_message.md> <thread_id> -`。
- 支持 Trae 常用执行参数：`model`、`profile`、`permissionMode`、`sandbox`、`dangerFullAccess`、`skipGitRepoCheck`、`ignoreUserConfig`、`ignoreRules`、`addDirs`、`configOverrides`、feature enable/disable、`outputSchema`、`color`、`ephemeral`。
- External CLI runtime 必须逐行读取 stdout/stderr，解析到 JSONL progress event 后立即向调用方推送，不能等 CLI 进程结束后才批量解析。
- Web Chat `/chat/stream` 与 slash runner-call stream 必须把 Trae progress event 实时写入同一份 conversation timeline。
- IM event loop 必须把 Trae progress event 同时推给 progress card 和 conversation timeline。
- WebUI Runners 配置面支持选择 Trae CLI，并在最终消息中默认展开已完成的过程块。

### 必须不破坏

- 不改变 Codex、ChatGPT Web、custom CLI 的 runner 选择语义、work_dir 继承和持久化会话恢复。
- 不把外层 `traex` wrapper 调用作为用户可见工具步骤写进 timeline；用户应该看到 Trae 自己输出的状态、工具和最终回答。
- 不伪造 Trae 没有输出的细粒度工具事件。若 Trae 本次 JSONL 只提供状态和最终消息，Bifrost 只展示真实状态和最终消息；若 Trae 输出 tool started/finished，Bifrost 再展示工具步骤。
- 不向 Trae exec 传递不支持的 `--permission-mode default`。配置为空或 `default` 时必须省略该参数，让 Trae 使用自身 headless 默认值。
- 不影响飞书 IM progress card 的最终收敛和普通 final reply 投递策略。

### 必须真实验证

- 使用真实 Trae CLI 执行一个 Web Chat turn，确认最终历史页显示 `Runner: traex`、过程块默认展开、最终答案可见。
- 使用真实飞书 IM 入站或已保存的飞书 session history 验证 Trae runner 输出进入同一条 timeline，并且 Web Chat 历史页能看到过程和最终结果。
- 验证 Trae `permissionMode` 空/default 配置不会生成 `--permission-mode default`，避免 exec 模式报错。
- 验证没有外层 wrapper 工具调用噪音，timeline 中只保留 Trae JSONL 归一化后的事件。

## 实现逻辑

### Adapter 命令构造

`command_spec.rs` 新增 `traex` adapter 分支。默认 args 为空时，运行时生成 Trae exec/resume 命令，并把 `--cd` 放在 `exec` 前面，确保 Trae 以目标工程目录启动。`--output-last-message` 继续使用 External CLI runtime 已有的 final response 文件契约。

Trae 的 `permissionMode` 与 Codex 的 approval policy 不同，因此单独映射为 `--permission-mode`。WebUI 把 “Headless default” 保存为空值，后端也把空字符串和 `default` 都视为省略。

### 实时输出

External CLI runtime 的旧路径会等子进程结束后再解析 stdout。新增 `run_with_progress()` 后，stdout/stderr 读任务逐行捕获输出；stdout 行会先尝试解析为 `ExternalCliProgressEvent`，解析成功即通过 channel 发出，同时仍写入 run artifact。worker 模式也新增 `ExternalCliWorkerEvent::Progress`，让主进程能在 worker 仍运行时收到事件。

这条路径保证 Web Chat、runner-call、IM event loop 都能以同一套 API 获取实时 progress，而不是分别重放 artifact。

### Timeline 与展示

Web Chat 和 IM event loop 使用 `record_external_cli_progress_event_to_timeline()` 将 external CLI event 转为 conversation recorder 事件：

- `RunStarted` / `RunFinished` / `RunFailed` 写入 run state。
- `Status` 与 `AssistantDelta` 写入 assistant delta。
- `ToolStarted` 写入 tool call。
- `ToolFinished` 写入 tool call + tool result，优先使用 Trae 原始事件中的 call id、tool name、arguments。
- `AssistantFinal` 不重复写入，最终回答仍由 run result 统一记录为 assistant message。

前端 `ProcessStepsBlock` 对已完成消息默认展开，turn group 只要包含过程步骤也默认展开。这样用户打开最终历史时可以直接看到过程信息，而不是只看到一条最终回答或折叠的 “Ran Xs”。

## 依赖项

- 本机已安装 `traex` CLI，并支持 `exec --json --output-last-message`。
- Bifrost 现有 `ExternalCliRuntime`、`ConversationRecorder`、IM progress card registry、Agent Chat timeline parser。
- WebUI Runners 配置和 Agent Chat 消息组件。

## 测试方案

### 单元测试

- `traex_adapter_builds_exec_command_with_prompt_stdin`：验证默认 Trae exec 命令和参数顺序。
- `traex_adapter_builds_resume_command_from_thread_id`：验证 threadId 续接命令。
- `traex_adapter_omits_default_permission_mode_for_exec`：验证空/default permission mode 不传给 Trae。
- `traex_cli_parser_maps_real_jsonl_events`：验证 Trae JSONL 事件可归一化。
- `external_cli_runtime_streams_stdout_before_process_exit`：验证 stdout 事件在进程退出前已经推送。
- `external_progress_maps_to_agent_turn_progress_events`：验证 external progress 可转 IM progress card 事件。
- `external_runner_progress_events_are_recorded_as_visible_timeline_steps`：验证 status/tool 事件写入可见 timeline。
- `assistant_final_is_pipeline_content_until_turn_finished`：验证 Trae/Codex 公开 `agent_message` 在 runner 仍运行时进入 Pipeline 过程，不提前占用底部最终结论。
- `timed_out_external_cli_result_reports_failure_reply`：验证 Trae 超时等非成功状态按失败收敛，不把早期 `agent_message` 当作成功结果。

### E2E 测试

- 使用临时 `BIFROST_DATA_DIR` 启动服务，配置 `traex` runner，调用 `/chat/stream`，断言 NDJSON 中包含 Trae progress event、最终 `run_finished`、run detail artifacts 和 timeline。
- 使用 WebUI 真实浏览器打开 Agent Chat，发送 Trae runner 消息，断言运行中可见 process，完成后过程块默认展开且最终回答可见。

### 真实场景测试

- 更新 `human_tests/im-gateway-external-cli-chat-gateway.md`，新增 Trae Web Chat、飞书 IM progress card、permission mode default 省略三个用例。
- 按用例真实执行：临时端口、临时数据目录、`--no-system-proxy`、禁用 Sync 自动登录弹窗。
- WebUI 亮色/暗色至少验证 Agent Chat 过程块可读性；本轮主要变更不新增硬编码主题色。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：Trae runner、实时输出、Web Chat 体验、飞书 IM progress card 体验、最终过程可见。
- Review 范围：`external_cli` runtime/command spec/parser、Chat Gateway stream、IM event loop、Agent Chat 前端、Runners 配置 UI、design/human_tests。
- 复测命令：focused Rust 单测、真实 WebUI Trae run、human_tests 新增用例。

### 第 2 轮

- 复查第 1 轮修复后的 diff，重点检查是否仍有 wrapper tool 噪音、final assistant 重复、Trae permission mode 错误、ChatGPT Web/Codex 回归。
- 复测命令：focused Rust 单测、`cargo test --workspace --all-features`、clippy/fmt、必要时 local-ci。

## 校验要求

- 先执行本模块相关 E2E 和 human_tests，再执行 rust-project-validate。
- 提交前至少执行一次 `cargo test --workspace --all-features`。
- WebUI 变更需要通过前端构建或由 `cargo run --bin bifrost -- start` 的前端构建阶段验证。

## 文档更新要求

- 更新本设计文档。
- 更新 `human_tests/im-gateway-external-cli-chat-gateway.md` 与 `human_tests/readme.md`。
- 如后续暴露 Trae adapter API 文档或 README runner 示例，再同步补充公开文档。
