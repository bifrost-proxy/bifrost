# Agent Codex Parity

## 功能模块说明

验证 Bifrost 内置 Agent 的 Codex parity 收敛：保留 Bifrost IM Agent 产品边界，引入 Codex 关键运行时契约；唯一安全例外是默认完全开放授权，不启用限制性 sandbox。

## 前置条件

- 工作目录：仓库根目录。
- 执行命令前先执行 `source ~/.zshrc`。
- 本用例不启动 Bifrost 服务，因此不需要系统代理；如后续扩展为真实服务用例，必须使用临时 `BIFROST_DATA_DIR` 且启动命令携带 `--no-system-proxy`。

## 测试用例列表

### TC-ACP-01 开放授权策略默认值

操作步骤：

1. 执行 `source ~/.zshrc && cargo test -p bifrost-agent authorization::tests::default_policy_is_open_without_sandbox_or_approval -- --exact`。
2. 执行 `source ~/.zshrc && cargo test -p bifrost-agent authorization::tests::orchestrator_allows_local_and_mcp_tools -- --exact`。

预期结果：

- 默认 permission profile 为 `Open`。
- approval policy 为 `NeverAsk`。
- sandbox policy 为 `Disabled`。
- local 与 MCP tool 都通过同一 ToolOrchestrator 放行。

### TC-ACP-02 Responses streaming 协议

操作步骤：

1. 执行 `source ~/.zshrc && cargo test -p bifrost-agent responses::tests::responses_url_converts_chat_completions_endpoint -- --exact`。
2. 执行 `source ~/.zshrc && cargo test -p bifrost-agent responses::tests::parses_responses_stream_text_and_function_call -- --exact`。
3. 执行 `source ~/.zshrc && cargo test -p bifrost-agent responses::tests::streaming_client_sends_responses_shape -- --exact`。

预期结果：

- `/v1/chat/completions` endpoint 被转换为 `/v1/responses`。
- SSE parser 能解析文本 delta、function_call 与 usage。
- streaming client 请求体使用 Responses `input`/`stream`/`reasoning` shape，不再发送 Chat Completions `messages` 或顶层 `reasoning_effort`。

### TC-ACP-03 Provider wire api 配置

操作步骤：

1. 执行 `source ~/.zshrc && cargo test -p bifrost-agent config::tests::test_resolve_effective_config_openai -- --exact`。

预期结果：

- OpenAI builtin provider resolves to `ModelWireApi::Responses`。
- AIDP 与其他 OpenAI-compatible provider 不受本用例影响，仍可保留 Chat Completions fallback。

### TC-ACP-04 Agent parity E2E 合约脚本

操作步骤：

1. 执行 `source ~/.zshrc && e2e-tests/tests/test_agent_codex_parity_contracts.sh`。

预期结果：

- `cargo check -p bifrost-agent --tests` 通过。
- 开放授权、Responses streaming、OpenAI provider wire api 的测试全部通过。
- 脚本输出 `[agent-codex-parity] OK`。

### TC-ACP-05 绝对目录输入切换工作路径回归

操作步骤：

1. 执行 `source ~/.zshrc && cargo test -p bifrost-agent absolute_directory_input_switches_work_dir_without_model_call`。
2. 执行 `source ~/.zshrc && cargo test -p bifrost-agent absolute_directory_path_detection_rejects_non_switch_inputs`。
3. 执行 `source ~/.zshrc && e2e-tests/tests/test_agent_direct_path_switch.sh`。

预期结果：

- 单元测试中，用户消息为真实存在的绝对目录路径时，Agent 直接切换 `session.work_dir`，返回 `work_dir_switched`，清空旧 history，且不调用 mock 模型。
- 单元测试中，相对路径、绝对目录后带参数、不存在的绝对路径和绝对文件路径不会被误判为工作路径切换请求。
- 真实服务 E2E 中，`POST /_bifrost/api/im-gateway/agent/chat` 第一次业务消息调用 mock provider；第二次发送绝对目录路径时返回“已切换工作目录到”，不返回“未知命令”，mock provider 仍只收到一次业务请求。
- 随后 `/status` 显示工作路径为新目录，证明工作目录切换已进入 session 状态而不是只返回提示文案。

## 清理步骤

- TC-ACP-01 至 TC-ACP-04 不创建服务进程或临时数据目录，无需额外清理。
- TC-ACP-05 的 E2E 脚本使用 `mktemp -d` 创建临时 `BIFROST_DATA_DIR`、工作目录和 mock 日志，退出时自动停止 Bifrost/mock provider 并删除临时目录；启动 Bifrost 时携带 `--no-system-proxy`。

## 执行记录

### 2026-05-22

- TC-ACP-01：已执行，通过。
- TC-ACP-02：已执行，通过。
- TC-ACP-03：已执行，通过。
- TC-ACP-04：已执行，通过。

### 2026-05-23

- TC-ACP-05：已执行，通过。`cargo test -p bifrost-agent absolute_directory_input_switches_work_dir_without_model_call` passed；`cargo test -p bifrost-agent absolute_directory_path_detection_rejects_non_switch_inputs` passed；`e2e-tests/tests/test_agent_direct_path_switch.sh` 输出 `[agent-direct-path-switch] PASS`。
