# External Runner 子 Agent 事件边界真实场景测试

## 功能模块说明

本模块验证 Codex、Trae X 与 Claude Code 对子 Agent 不做专属生命周期解析。根 Agent 的协作动作只作为普通工具输入/输出展示；子线程的消息、工具、状态和完成事件不得进入根事件流，不得提前结束根任务或把子 Agent 内容渲染成飞书最终卡片。升级前持久化的 `subagent_updated` 仅保留历史兼容展示。

## 前置条件

1. 当前目录位于仓库根目录，并已构建当前分支的 `target/debug/bifrost`。
2. 自动回归使用隔离的临时数据目录、动态端口和 mock runner，不修改用户现有 Bifrost 服务或系统代理。
3. 真实 Runner 回归需要本机可用且已认证的 Trae X 与 Claude Code CLI；本机路径分别通过 `BIFROST_TRAEX_BIN`、`BIFROST_CLAUDE_BIN` 显式传入。
4. 真实 Runner 只读取 `design/external-runner-subagent-event-boundary.md`，不授权写文件。

## 测试用例列表

### TC-ERSO-01：三 Runner 服务级确定性事件边界

**操作步骤**：

1. 执行：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_im_gateway_subagent_event_boundary.sh
   ```
2. 等待脚本依次通过 Codex app-server、Trae X app-server 与 Claude Code stream-json 三次 `/chat` 调用。

**预期结果**：

- Codex 与 Trae X 在 mock 子 thread 输出消息、工具和 `turn/completed` 后仍继续处理根 turn，最终响应为 `ROOT_FINAL_OK`。
- 两者都只在根 `turn/completed` 产生一次 `run_finished`；协作调用恰好是一组普通 `tool_started` / `tool_finished`。
- Claude Code 的 `Task` 也只产生普通工具输入/输出，最终响应为 `ROOT_CLAUDE_FINAL_OK`。
- 三条链路均不产生新的 `subagent_updated`，事件 JSON 不包含 mock 子消息、子工具输出、子 thread ID 或 `agentsStates` 内部详情。
- 三次调用复用的隔离 Bifrost 服务持续存活，脚本输出 `[im-gateway-subagent-event-boundary] PASS` 后清理临时进程和目录。

**实际结果（2026-08-11）**：通过。执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_im_gateway_subagent_event_boundary.sh`，Codex、Trae X、Claude Code 三次隔离 `/chat` 调用全部满足断言，脚本输出 `[im-gateway-subagent-event-boundary] PASS`；mock 子消息、子工具、子 thread ID、内部 `agentsStates` 和 `subagent_updated` 均未进入根事件流。

### TC-ERSO-02：真实 Trae X 子协作完成不结束根任务

**操作步骤**：

1. 确认真实 CLI：
   ```bash
   /Users/eden/.local/bin/traex --version
   ```
2. 执行真实 Trae X runner 模式：
   ```bash
   RUN_REAL_TRAEX_SUBAGENT_E2E=true SKIP_BUILD=true \
   BIFROST_BIN="$PWD/target/debug/bifrost" \
   BIFROST_TRAEX_BIN=/Users/eden/.local/bin/traex \
     bash e2e-tests/tests/test_im_gateway_subagent_event_boundary.sh
   ```
3. 脚本要求根 Trae X 派发一个只读子任务、等待子任务读出设计文档标题，然后根 Agent 回复 `ROOT_TRAEX_SUBAGENT_OK`。

**预期结果**：

- 真实 Trae X run 成功，响应包含 `ROOT_TRAEX_SUBAGENT_OK`。
- 事件流存在协作工具的普通开始和完成事件，不存在 `subagent_updated`。
- 子协作完成后根 Agent 仍继续运行并输出 marker；只在根 turn 结束时产生一次 `run_finished`。
- 同一隔离 Bifrost 服务在 Trae X run 后仍健康，并继续执行 Claude Code run。

**实际结果（2026-08-11）**：通过。真实 Trae X `0.200.19` run `1786377345111-d9119b09-b90c-48c1-b6a6-2a8bd387fb22` 完成一次子协作后，根 Agent 继续返回 `ROOT_TRAEX_SUBAGENT_OK`；脚本确认协作仅为普通工具开始/完成、无 `subagent_updated`、根 `run_finished` 只有一次，并在完成后通过服务健康检查。

### TC-ERSO-03：真实 Claude Code Task/Agent 使用普通工具边界

**操作步骤**：

1. 确认真实 CLI：
   ```bash
   /Users/eden/work/code/next-harness-polish/node_modules/@anthropic-ai/claude-agent-sdk-darwin-arm64/claude --version
   ```
2. 执行真实 Claude Code runner 模式：
   ```bash
   RUN_REAL_CLAUDE_SUBAGENT_E2E=true SKIP_BUILD=true \
   BIFROST_BIN="$PWD/target/debug/bifrost" \
   BIFROST_CLAUDE_BIN=/Users/eden/work/code/next-harness-polish/node_modules/@anthropic-ai/claude-agent-sdk-darwin-arm64/claude \
     bash e2e-tests/tests/test_im_gateway_subagent_event_boundary.sh
   ```
3. 脚本要求根 Claude Code 使用一次 `Agent` 或 `Task` 工具、等待只读子任务返回设计文档标题，再回复 `ROOT_CLAUDE_SUBAGENT_OK`。

**预期结果**：

- 真实 Claude Code run 成功，响应包含 `ROOT_CLAUDE_SUBAGENT_OK`。
- `Task` / `Agent` 只产生普通 `tool_started` / `tool_finished`，输入保留 prompt，完成事件保留结果与 success。
- 不产生新的 `subagent_updated`，子任务结束不会提前结束根任务；根 run 只产生一次 `run_finished`。
- 两个真实 run 完成后隔离 Bifrost 健康接口仍返回 200。

**实际结果（2026-08-11）**：环境阻塞。真实 Claude Code `2.1.186` 已由隔离 Bifrost 正常启动，run `1786377279463-c06823ac-5b12-4986-afcc-ded2e092cf58` 进入 stream-json 并返回明确认证错误 `Not logged in · Please run /login`；重新加载 `~/.zshrc` 后确认 `ANTHROPIC_API_KEY`、`CLAUDE_CODE_OAUTH_TOKEN` 与 AWS 认证变量均未设置，因此无法真实触发 `Task`。同一实现已由 TC-ERSO-01 的真实 Bifrost 进程 + Claude stream-json mock 完整验证普通 `Task` 输入/输出与根完成边界；真实账号链路仍需登录后复跑本用例，未误报为通过。

## 清理步骤

1. 测试脚本通过 trap 停止隔离 Bifrost 进程并删除临时数据目录。
2. 确认没有遗留 `mock-app-server`、`mock-claude` 或真实 runner 子进程。
3. 测试始终使用 `--no-system-proxy`，不修改用户系统代理设置。
