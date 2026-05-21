# Agent Custom Runner / Codex CLI Chat Gateway 技术方案

## 背景

IM Gateway 接入 Codex CLI 后，真实 IM 入站链路仍然是最终验收目标，但开发、E2E 和回归验证不能依赖人工从飞书或微信发送消息。需要补充一个 Chat Gateway，让开发者和测试脚本可以直接通过 HTTP/API 发起同等语义的消息，复用 IM Gateway 的消息清洗、会话调度、外部 CLI Agent runtime、进度事件监听和 IM 风格渲染逻辑。Codex 是首个目标 CLI，但架构必须支持后续平滑接入其他同类 CLI Agent。

Chat Gateway 不是新的 Agent 产品形态，也不是绕过 IM Gateway 的快捷入口。它是 IM Gateway 的 provider-neutral 测试/调试入口，目标是把“IM 入站事件”替换成“可构造的 Chat 请求”，其余执行链路保持一致。

## 用户目标验证清单

### 必须实现

- 提供 HTTP Chat Gateway，可直接调用接口触发外部 CLI Agent 执行，Codex CLI 是首个内置 adapter。
- Chat Gateway 请求经过与 IM 入站一致的消息清洗、session key 解析、队列/busy 处理和 progress event 渲染。
- 支持同步返回最终结果，也支持流式读取进度事件，方便自动化测试断言。
- 支持注入工程目录、route/provider/global 指令、skill roots、Bifrost 工具集合和 adapter-specific CLI 执行参数。
- 支持构造 IM 上下文元数据，例如 provider、chat_id、user_id、message_id、reply target，用于验证 send/update 消息类工具。

### 必须不破坏

- 不改变现有 Feishu/Weixin 长连接入站处理。
- 不绕过 Remote Invoke / IM Gateway 已有权限边界；测试入口必须显式标记为本地/admin 调试能力。
- 不把测试请求中的任意 work_dir 直接放开到外部 CLI Agent；必须经过 allowlist 或配置解析。
- 不让 Chat Gateway 和 IM Gateway 各自实现两套 message sanitizer、progress renderer 或 session queue。
- 不把 secret、provider token、完整 provider config 暴露到 Chat Gateway 响应。

### 必须真实验证

- 通过真实 HTTP 请求触发外部 CLI Agent runtime，不依赖人工 IM 消息。
- 断言 Codex CLI JSONL 输出被 adapter 转换为 Bifrost `AgentTurnProgressEvent`；后续其他 CLI 输出也必须进入同一 canonical event 模型。
- 断言最终消息与流式事件都可被测试脚本读取。
- 断言模拟 IM 上下文下，send/update 消息工具使用受控目标而不是泄漏到真实 IM。

## 架构原则

### 1. 一个 runtime，两个入口

入口分为真实 IM 和 Chat Gateway，但二者在 sanitizer 之后进入同一个 runtime pipeline。

```text
Feishu / Weixin Event                         HTTP Chat Gateway
        |                                             |
        v                                             v
Provider Event Normalizer                    Chat Request Normalizer
        |                                             |
        +--------------------+------------------------+
                             |
                             v
                    MessageSanitizer
                             |
                             v
                    SessionQueueManager
                             |
                             v
                       AgentRuntime
           BifrostAgent | ExternalCliAgent
                       /        |        \
                  Codex CLI  Claude CLI  Other CLI
                             |
                             v
                 AgentTurnProgressEvent
                             |
                             v
               Progress Renderer / Test Stream
```

### 2. Chat Gateway 只构造入站上下文

Chat Gateway 负责把 HTTP 请求转换成 `NormalizedInboundMessage`，字段包括：

- `source_kind`: `im` 或 `chat_gateway`
- `provider_id`: 可选，使用真实 provider 配置时填写
- `provider_type`: `feishu` / `weixin` / `mock`
- `chat_id`: 测试 chat/thread 标识
- `user_id`: 测试用户标识
- `message_id`: 请求侧指定或服务端生成
- `text`: 清洗后的用户文本
- `images`: 可选多模态图片
- `reply_mode`: `none` / `test_stream` / `real_im`
- `target`: 可选 IM target binding

后续 session key、busy 队列、/status、/stop、/clear、progress card snapshot 都不区分消息来自真实 IM 还是 Chat Gateway。

### 3. 测试流与真实 IM 渲染解耦

真实 IM 渲染走 provider 的 `send_text` / `send_card` / `patch_card`。Chat Gateway 默认不向真实 IM 发消息，而是把同一组 progress event 暴露给调用方：

- 同步模式：请求阻塞到 turn 结束，返回 final response、工具日志、进度快照。
- 流式模式：返回 SSE 或 NDJSON，逐条输出 `AgentTurnProgressEvent` 映射后的测试事件。
- 回放模式：给定 `run_id`，读取历史 JSONL / event log，便于失败后定位。

只有当请求显式设置 `reply_mode=real_im` 且调用方具备 admin 权限时，才允许把结果发送到真实 provider/target。

### 4. Runtime adapter 插件化

架构上不要把 Codex CLI 写成 IM Gateway 的特殊分支。Codex CLI 应该只是第一个 `ExternalCliAgentAdapter`，后续接入 Claude Code、Gemini CLI、Trae CLI、Cursor Agent 或内部 CLI Agent 时，只需要新增 adapter，不改 IM event loop、session queue、progress renderer 和 Chat Gateway API。

核心抽象：

```rust
trait AgentRuntime {
    fn runtime_id(&self) -> &'static str;
    async fn run_turn(
        &self,
        input: AgentRunInput,
        sink: AgentProgressSink,
    ) -> Result<AgentRunResult>;
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

其中：

- `AgentRuntime` 是 IM Gateway 看到的稳定接口。
- `ExternalCliAgentRuntime` 是通用进程托管 runtime，负责 run dir、进程生命周期、stdout/stderr、超时、stop、artifact、事件落盘。
- `ExternalCliAgentAdapter` 只负责某个 CLI 的差异：命令参数、prompt envelope、stdout 事件解析、最终答案提取。
- Codex adapter 实现 `codex exec --json`；其他 CLI adapter 可以实现自己的 JSON、SSE、纯文本或文件输出解析。

### 5. Canonical progress event

所有 runtime 都必须归一化为 Bifrost 的 canonical progress event，而不是让 WebUI 或 IM renderer 理解每个 CLI 的私有事件。

建议事件模型：

```text
RunStarted
StatusChanged
AssistantDelta
AssistantFinal
PlanUpdated
ToolStarted
ToolFinished
ArtifactCreated
RunFinished
RunFailed
RunStopped
```

adapter 允许输出能力缺失。例如某个 CLI 不提供 tool started 事件，就只输出 assistant delta 和 final；UI 仍然能正常展示，只是工具面板为空。

### 6. 能力声明与降级

每个 adapter 需要声明能力，WebUI 和 API 根据能力展示配置项：

```json
{
  "adapter_id": "codex",
  "display_name": "Codex CLI",
  "capabilities": {
    "json_events": true,
    "resume": true,
    "images": true,
    "sandbox": true,
    "approval_policy": true,
    "mcp": true,
    "skills": true,
    "output_last_message_file": true
  }
}
```

配置 UI 不能假设所有 CLI 都支持 Codex 的 `sandbox`、`approval_policy`、`--image`、`--json` 或 `--output-last-message`。不支持的能力要隐藏或只读显示为“不适用”；运行时必须记录降级原因到 `runtime_snapshot.json`。

### 7. CLI adapter 注册

V1 可以内置注册：

- `bifrost_agent`：现有内嵌 Bifrost Agent runtime。
- `codex`：Codex CLI adapter。

V2 再开放本地 adapter manifest：

```toml
[[agent.external_cli.adapters]]
id = "my-agent"
display_name = "My Agent CLI"
command = "my-agent"
args = ["run", "--json"]
event_format = "jsonl"
final_response = { type = "file", path = "final.md" }
```

manifest adapter 适合简单 CLI；复杂 CLI 仍通过 Rust 内置 adapter 实现 parser 和能力声明。

## API 设计

### `POST /_bifrost/api/im-gateway/chat`

同步执行一个 turn，默认等待最终结果。

请求：

```json
{
  "message": "帮我查看最近 10 条 Bifrost traffic 并总结异常",
  "session_key": "debug-local-traffic",
  "runtime": "external_cli",
  "adapter": "codex",
  "provider_id": "bifrost",
  "provider_type": "feishu",
  "chat_id": "debug-chat",
  "user_id": "debug-user",
  "message_id": "debug-msg-001",
  "work_dir": "~/work/github/bifrost",
  "reply_mode": "test_stream",
  "instructions": {
    "developer": "用中文输出，结论简洁，保留可复查命令。",
    "user": "这是 Chat Gateway 调试入口，不要等待真实 IM 输入。"
  },
  "adapter_config": {
    "adapter_id": "codex",
    "profile": "bifrost-im",
    "model": "gpt-5.4",
    "sandbox": "workspace-write",
    "approval_policy": "never",
    "search": false,
    "ephemeral": false
  },
  "skills": {
    "enabled": ["bifrost", "bifrost-remote"],
    "disabled": []
  },
  "images": []
}
```

响应：

```json
{
  "ok": true,
  "run_id": "run_abc123",
  "session_key": "debug-local-traffic",
  "runtime": "external_cli",
  "adapter": "codex",
  "status": "completed",
  "response": "已检查最近 10 条 traffic...",
  "title": "检查 Bifrost traffic",
  "work_dir": "~/work/github/bifrost",
  "tool_calls": [
    {
      "tool_name": "exec_command",
      "success": true,
      "duration_ms": 1240,
      "result_preview": "..."
    }
  ],
  "progress_snapshot": {
    "phase": "finished",
    "plan_steps": [],
    "latest_tool": null
  },
  "artifacts": {
    "stdout": ".../runs/run_abc123/cli.stdout.log",
    "stderr": ".../runs/run_abc123/cli.stderr.log",
    "last_message": ".../runs/run_abc123/last_message.md"
  }
}
```

### `POST /_bifrost/api/im-gateway/chat/stream`

流式执行一个 turn。推荐使用 NDJSON，因为它和 `codex exec --json` 天然匹配，测试脚本也容易消费。

事件示例：

```json
{"type":"run_started","run_id":"run_abc123","session_key":"debug-local-traffic"}
{"type":"status","stage":"model_request","iteration":1}
{"type":"assistant_delta","content":"我先查看最近 traffic..."}
{"type":"tool_started","tool_name":"exec_command","arguments":"..."}
{"type":"tool_finished","tool_name":"exec_command","success":true,"duration_ms":1240,"result_preview":"..."}
{"type":"assistant_final","content":"已检查..."}
{"type":"run_finished","status":"completed","response":"已检查..."}
```

### `GET /_bifrost/api/im-gateway/chat/runs/:run_id`

查询 run 详情，返回最终状态、progress snapshot、stdout/stderr/last_message 路径和安全摘要。

### `GET /_bifrost/api/im-gateway/chat/runs/:run_id/events`

读取规范化后的 progress event log，用于 E2E 失败后回放。

### `POST /_bifrost/api/im-gateway/chat/runs/:run_id/stop`

停止正在运行的外部 CLI Agent 进程，并把 session 状态更新为 stopped。该接口应复用 IM `/stop` 的 session control 语义。

## Runtime 配置合并

Agent 模式配置必须同时支持全局默认配置和单个 IM 通道配置。全局配置描述“默认使用哪种 Agent runtime、外部 CLI Agent 如何启动、默认工程目录和默认工具/skill 能力”；通道配置描述“这个 provider/channel 是否启用 Agent、使用哪个目录、追加什么指令、允许哪些能力”。Chat Gateway 和真实 IM 入站必须使用同一套合并逻辑。

### 配置层级

```text
Global Agent Defaults
        |
        v
IM Provider / Channel Agent Config
        |
        v
Route / Schedule / Chat Gateway Request Override
        |
        v
Single Run Runtime Snapshot
```

合并顺序：

1. 全局 `agent.external_cli` / `agent.adapters`
2. provider/channel 的 `agent_config`
3. route、schedule 或 Chat Gateway request override
4. 单次 run 的临时字段

后一级只覆盖显式设置的字段；未设置字段继承上一级默认值。单次请求 override 只对当前 run 生效，不能自动写回 provider/channel/global 配置。最终执行前必须生成不可变的 `AgentRuntimeSnapshot` 并落盘到 run dir，便于审计和复现。

### 全局默认配置

全局默认配置建议存储在 `$BIFROST_DATA_DIR/agent/config.toml` 或现有 Agent config store 中，作为所有 IM 通道和 Chat Gateway 的默认值。

```toml
[agent]
enabled = true
default_runtime = "external_cli" # bifrost_agent | external_cli
default_adapter = "codex"

[agent.external_cli]
enabled = true
work_dir = "~/work/github/bifrost"
timeout_secs = 3600
ephemeral = false

allowed_work_dirs = [
  "~/work/github/bifrost"
]
add_dirs = [
  "~/.codex/skills",
  "~/.agents/skills",
  "~/.bifrost/agent/skills"
]

[agent.adapters.codex]
enabled = true
executable = "codex"
profile = "bifrost-im"
model = "gpt-5.4"
sandbox = "workspace-write"
approval_policy = "never"
search = false

[agent.instructions]
developer = "你是通过 Bifrost IM Gateway 调度的 Codex Agent。"
user = ""

[agent.skills]
include_repo = true
include_global = true
include_bifrost_system = true
enabled = ["bifrost", "bifrost-remote"]
disabled = []

[agent.message]
default_reply_mode = "test_stream" # none | test_stream | real_im
progress_update_interval_ms = 1200
max_final_message_chars = 12000
```

全局配置适合放通用能力：

- 默认 runtime、默认 adapter、外部 CLI 进程生命周期、超时和 artifact 策略。
- 各 adapter 的可执行文件、profile、默认模型和 adapter-specific 参数，例如 Codex 的 sandbox 和 approval policy。
- 默认工程目录和 allowlist roots。
- Bifrost 自带 skill roots 和默认启用的 skills。
- 默认开发者指令，例如 IM 场景输出风格、不要泄露 secret、长任务持续更新进度。
- 默认 progress 更新节流、最终消息长度、run artifact TTL。

### 单个 IM 通道配置

每个 IM provider/channel 可以配置自己的 Agent 模式。这里的 channel 指 Feishu provider、Weixin provider，或后续 provider 下更细的 chat/thread 绑定；V1 可先落在 provider 的 `agent_config` 上，V2 再扩展到 target/chat 级配置。

```toml
[[im.providers]]
id = "feishu-sre"
provider_type = "feishu"
enabled = true

[im.providers.agent]
enabled = true
runtime = "external_cli"
adapter = "codex"
work_dir = "~/work/code/nextoncall/next_agent"
allowed_work_dirs = [
  "~/work/code/nextoncall/next_agent",
  "~/work/code/nextoncall/next_agent/oncall"
]

[im.providers.agent.adapters.codex]
profile = "nextoncall-im"
model = "gpt-5.4"
sandbox = "workspace-write"
approval_policy = "never"

[im.providers.agent.instructions]
developer = "这个通道用于 NextOnCall 工程任务。优先读取仓库 AGENTS.md，真实验证后再下结论。"
user = ""

[im.providers.agent.skills]
enabled = ["bifrost", "bifrost-remote", "site-cookie-login"]
disabled = ["imagegen"]

[im.providers.agent.message]
default_reply_mode = "real_im"
progress_card = true
```

通道配置适合覆盖：

- `enabled`：该 IM 通道是否允许 Agent 模式。
- `runtime` / `adapter`：该通道使用内嵌 `bifrost_agent`，还是通过 `external_cli` 调度某个 CLI adapter。
- `work_dir` / `allowed_work_dirs`：该通道默认工程目录与可切换目录。
- `instructions`：该通道特有的工程背景、输出风格、验证要求。
- `skills.enabled/disabled`：该通道允许或禁用的 skill。
- `adapters.<id>`：该通道特定的 adapter 参数，例如 Codex 的 profile/model/sandbox/approval_policy。
- `message.default_reply_mode/progress_card`：该通道默认是否真实回 IM、是否使用进度卡。

### 字段合并规则

| 字段类型 | 合并规则 |
| --- | --- |
| 标量字段，如 `runtime`、`adapter`、`model`、`work_dir` | 后一级非空值覆盖前一级 |
| 指令字段 `developer` / `user` | 默认追加：global 在前，channel/route/request 在后；显式 `replace_*` 才替换 |
| `allowed_work_dirs` | 默认取交集或受 global allowlist 约束的子集，禁止 channel 扩大到 global 未允许目录 |
| `add_dirs` | 默认并集，但每项必须通过 global allowlist |
| `skills.enabled` | 后一级可追加启用，但不能启用 global policy 禁止的 skill |
| `skills.disabled` | 后一级追加禁用，禁用优先级最高 |
| `mcp_servers` | 默认按 server name 合并；后一级只能覆盖允许字段，secret/env 仍来自安全存储 |
| `reply_mode=real_im` | 必须同时满足 global 允许、channel 允许、request 显式需要和权限校验 |

### API 表达

全局配置接口：

- `GET /_bifrost/api/im-gateway/agent/defaults`
- `PATCH /_bifrost/api/im-gateway/agent/defaults`

通道配置接口：

- `GET /_bifrost/api/im-gateway/providers/:provider_id/agent`
- `PATCH /_bifrost/api/im-gateway/providers/:provider_id/agent`

预览有效配置接口：

- `POST /_bifrost/api/im-gateway/agent/resolve-config`

`resolve-config` 输入 provider、route、work_dir 和 request overrides，返回脱敏后的 `AgentRuntimeSnapshot`，用于 WebUI 预览和 E2E 断言。该接口不返回 secret、token 或完整 provider config。

### 运行时快照

每次执行前生成 `runtime_snapshot.json`：

```json
{
  "runtime": "external_cli",
  "adapter": "codex",
  "provider_id": "feishu-sre",
  "session_key": "feishu-sre:debug-user",
  "work_dir": "~/work/code/nextoncall/next_agent",
  "external_cli": {
    "run_dir": ".../chat_runs/run_abc123",
    "timeout_secs": 3600
  },
  "adapter_config": {
    "adapter_id": "codex",
    "executable": "codex",
    "profile": "nextoncall-im",
    "model": "gpt-5.4",
    "sandbox": "workspace-write",
    "approval_policy": "never"
  },
  "instructions_sources": [
    "global.agent.instructions.developer",
    "provider.feishu-sre.agent.instructions.developer",
    "chat_gateway.request.instructions.developer"
  ],
  "skills": [
    {
      "name": "bifrost",
      "scope": "global",
      "path": "~/.agents/skills/bifrost/SKILL.md"
    }
  ],
  "reply_mode": "test_stream"
}
```

这样 `/chat`、真实 IM 入站和 schedule 都能用同一个快照执行，测试失败时也能直接复现当时的 agent 模式配置。

## 工程目录注入

External CLI runtime 负责解析工程目录，adapter 再把它映射到具体 CLI 参数。例如 Codex adapter 使用 `--cd <resolved_work_dir>`，其他 CLI 可能使用 `--cwd`、`--project` 或环境变量。解析规则：

- 如果 request 提供 `work_dir`，必须在 `allowed_work_dirs` 中，或位于某个 allowlist root 下。
- 如果 request 未提供，则使用 provider/route/global 默认目录。
- 如果仍为空，则拒绝执行，并返回明确错误。
- `--add-dir` 仅用于 skill roots、只读参考目录或配置允许的辅助目录。

示例配置：

```toml
[agent.external_cli]
work_dir = "~/work/github/bifrost"
allowed_work_dirs = [
  "~/work/github/bifrost",
  "~/work/code/nextoncall/next_agent"
]
add_dirs = [
  "~/.codex/skills",
  "~/.agents/skills",
  "~/.bifrost/agent/skills"
]
```

## 指令注入

Chat Gateway 生成一次性 prompt envelope，而不是修改仓库 `AGENTS.md`。

```md
# Bifrost Chat Gateway Context

Runtime: external_cli
Adapter: codex
Source: chat_gateway
Provider: feishu
Chat ID: debug-chat
User ID: debug-user
Message ID: debug-msg-001
Work Dir: ~/work/github/bifrost

# Runtime Instructions

<global/provider/route/request developer instructions>

# Available Skills

<skill name + description + absolute SKILL.md path>

# User Message

<cleaned message>
```

具体 CLI 原生仍可读取自己的全局配置、profile 和项目指令文件。以 Codex 为例，它仍会读取用户全局配置、profile 和项目 `AGENTS.md`。Bifrost 注入的 prompt 只表达 IM/Chat Gateway 场景上下文、路由策略和 skill metadata。

## Skill 注入

Chat Gateway 必须和真实 IM Agent 使用相同 skill resolver。

- Repo skill: `<work_dir>/.agents/skills`
- Global skill: `~/.agents/skills`
- Codex skill: `~/.codex/skills`
- Bifrost user/system skill: `$BIFROST_DATA_DIR/agent/skills` 与 `.system`

注入方式：

1. 解析有效 skill 列表，按 scope priority 去重。
2. prompt 中只放 name、description、绝对路径。
3. 通过 adapter-specific 参数允许 CLI 读取 skill root；Codex adapter 使用 `--add-dir`。
4. 不 eager 注入完整 `SKILL.md`，避免 prompt 膨胀。
5. Bifrost 内置能力优先通过 `bifrost install-skill -t codex -y` 安装为 Codex 原生 skill；后续可升级为 MCP 工具。

## Bifrost 工具集合

### V1：CLI + Skill

外部 CLI Agent 通过 Bifrost skill 调用真实 `bifrost` CLI。Codex adapter 复用 Codex 原生 skill 目录；其他 adapter 需要声明自己支持的 skill 目录或 prompt 注入方式。例如：

- `bifrost traffic list/search/get`
- `bifrost im send`
- `bifrost im provider status`
- `bifrost remote ...`
- `bifrost rules ...`

Chat Gateway 测试默认不允许 `bifrost im send` 发到真实 IM，除非 `reply_mode=real_im` 且 admin 授权通过。

### V2：Bifrost MCP Server

后续新增 `bifrost mcp-server`，把高价值能力结构化暴露给 Codex：

- `im_send`
- `im_update_progress`
- `traffic_search`
- `traffic_get`
- `rule_list`
- `remote_status`
- `remote_invoke`
- `schedule_create`
- `schedule_update`
- `schedule_delete`

Chat Gateway 可通过 adapter-specific 配置注入该 MCP server。Codex adapter 可用 `codex -c mcp_servers.bifrost...` 或 profile 注入；其他 adapter 按各自 MCP 配置方式映射。

## External CLI 进程与事件监听

每个 Chat Gateway run 创建独立 run dir。run dir 命名不包含具体 adapter，方便不同 CLI 共用同一套查询和回放逻辑：

```text
$BIFROST_DATA_DIR/im_gateway/chat_runs/<run_id>/
  request.json
  prompt.md
  cli.stdout.log
  cli.stderr.log
  normalized_events.jsonl
  last_message.md
  runtime_snapshot.json
  meta.json
```

Codex adapter 的执行命令示例：

```bash
codex exec \
  --json \
  --cd "$WORK_DIR" \
  --sandbox workspace-write \
  --ask-for-approval never \
  --output-last-message "$RUN_DIR/last_message.md" \
  - < "$RUN_DIR/prompt.md"
```

其他 adapter 只需要实现 `build_command()`，输出统一的 `CommandSpec`：

```json
{
  "program": "other-agent",
  "args": ["run", "--json", "--cwd", "/repo"],
  "env": {
    "BIFROST_RUN_ID": "run_abc123"
  },
  "stdin_file": ".../prompt.md"
}
```

监听策略：

- stdout 先原样落盘，再交给 adapter parser；无法识别的行落盘但不阻断。
- stderr 落盘并节流输出摘要。
- parser 把 CLI 私有 event 转为 Bifrost `AgentTurnProgressEvent`。
- final response 由 adapter 决定提取策略；Codex adapter 优先用 `last_message.md`，如果文件为空，再从最后一个 assistant/final event 兜底。
- 进度事件进入 `ImAgentProgressRegistry` 和 Chat Gateway stream。

## WebUI 配置体验

WebUI 需要把 Agent 模式配置做成“全局默认 + 通道覆盖 + 有效配置预览 + 一键测试”的闭环，而不是把 TOML/JSON 字段直接铺给用户。目标是让用户能清楚知道当前 IM 通道会用哪个 runtime、在哪个工程目录执行、继承了哪些默认值、覆盖了哪些字段，以及真实执行前的最终快照是什么。

### 信息架构

建议在 Settings 的一级 `IM Gateway` 下保留现有 provider/target/route/schedule 结构，并增加两个配置入口：

1. `Agent Defaults`：全局默认 Agent 配置。
2. provider detail 的 `Agent` tab：单个 IM 通道的 Agent 覆盖配置。

如果后续支持 target/chat 级覆盖，可在 provider `Agent` tab 内增加 `Channel Overrides` 表格，按 chat/thread 细分；V1 不必急着做 target 级配置，避免配置层级过早复杂化。

```text
Settings
└── IM Gateway
    ├── Connections
    │   └── Provider Detail
    │       ├── Overview
    │       ├── Agent
    │       └── Permissions
    ├── Agent Defaults
    ├── Targets
    ├── Routes
    ├── Schedules
    ├── Chat Gateway
    └── Runs
```

### Agent Defaults 页面

全局默认页用于配置大多数通道都会继承的能力，布局建议采用“左侧分组导航 + 右侧表单 + 底部 sticky actions”。

分组：

- `Runtime`：启用状态、默认 runtime、默认 adapter、adapter executable、profile、model、timeout、ephemeral，以及 adapter-specific 配置。
- `Workspace`：默认 work_dir、allowed work dirs、add dirs。
- `Instructions`：developer instructions、user instructions；支持多行编辑和 Markdown 预览。
- `Skills`：include repo/global/bifrost system、enabled skills、disabled skills、skill roots 状态。
- `MCP / Tools`：Bifrost CLI skill 状态、后续 MCP server 配置和可用性检查。
- `Messaging`：默认 reply mode、progress 更新间隔、最终消息长度、artifact TTL。
- `Policy`：是否允许通道启用 `real_im`、是否允许通道扩展 work_dir、是否允许 request override。

每个字段需要显示继承/覆盖语义：

- 全局默认页显示“全局值”。
- 通道页显示“Inherited from global”或“Overridden”。
- 对危险字段显示解释和状态徽标，例如 `approval_policy=never`、`reply_mode=real_im`。

### 单通道 Agent 页面

provider detail 的 `Agent` tab 聚焦“这个 IM 通道和全局默认有什么不同”。建议顶部用摘要条展示有效配置：

```text
Agent: Enabled
Runtime: external_cli
Adapter: codex
Work dir: ~/work/code/nextoncall/next_agent
Reply: real_im with progress card
Skills: 3 enabled, 1 disabled
Policy: work_dir constrained by global allowlist
```

表单分区：

- `Mode`：启用/禁用 Agent、runtime、adapter、是否继承全局 runtime/adapter。
- `Workspace`：默认 work_dir、可切换目录；显示 global allowlist 命中状态，不允许保存越界路径。
- `Adapter`：profile、model 和 adapter-specific 参数。选择 Codex 时显示 sandbox、approval policy、search；其他 CLI 只显示其声明支持的字段。
- `Instructions`：追加 developer/user instructions，提供“replace global instructions”高级开关，但默认是追加。
- `Skills`：通道追加 enabled/disabled，禁用优先；展示最终 skill 清单和来源 scope。
- `Messaging`：是否真实回 IM、是否启用 progress card、默认 target mode。

通道页必须提供三个动作：

- `Preview Effective Config`：调用 `resolve-config`，展示脱敏 `AgentRuntimeSnapshot`。
- `Run Test Message`：打开 Chat Gateway 测试抽屉，使用当前通道配置发起测试。
- `Reset Overrides`：清除通道覆盖，回到全局继承。

### Chat Gateway 测试抽屉

Chat Gateway 不应只给 curl。WebUI 需要一个测试抽屉，让用户在保存配置前/后都能直接跑一条消息。

输入：

- provider/channel 选择器。
- session key 或自动生成。
- message 文本框。
- work_dir 选择器，仅列出允许目录。
- reply mode：默认 `test_stream`，`real_im` 放到高级区并要求二次确认。
- runtime/adapter override：默认折叠，只用于临时测试。

输出：

- 左侧显示流式事件时间线：run started、assistant delta、tool started、tool finished、final。
- 右侧显示当前 progress card 预览，复用 IM progress renderer 的数据结构。
- 底部显示 final response、tool calls、artifact links 和 runtime snapshot。

测试抽屉保存 run id，失败时提供 `Open Run Detail`，直接跳到 Runs 页面。

### Runs 页面

Runs 页面用于排查 Chat Gateway 和真实 IM Agent 执行。列表字段：

- run id、source kind、provider/channel、session key、runtime、adapter、status、title、work_dir、started_at、duration。
- final response preview。
- error preview。
- artifact availability。

详情页分区：

- `Timeline`：规范化 progress events。
- `Final Response`：最终输出。
- `Tools`：工具调用日志。
- `Runtime Snapshot`：脱敏快照。
- `Artifacts`：stdout JSONL、stderr、last_message、prompt envelope 的安全预览。
- `Raw Request`：脱敏请求。

真实 IM 入站和 Chat Gateway 测试 run 都进入同一个 Runs 页面，通过 `source_kind` 区分。

### 配置校验与保存体验

保存前必须做本地和服务端双重校验：

- work_dir 必须在全局 allowlist 内。
- add_dirs 必须在允许路径内。
- `reply_mode=real_im` 需要 global policy 和通道 permission 都允许。
- provider secret、token、完整 config 不在表单中展示。
- adapter executable 可以点 `Check`，执行只读可用性检查；Codex adapter 示例为 `codex --version`。
- MCP/skill 可以点 `Refresh`，查看可发现技能和工具数量。

保存后不直接执行任何 Agent 任务；用户需要点 `Run Test Message` 才触发 Chat Gateway。

### 前端组件建议

- 使用 segmented control 选择 runtime，用 select/combobox 选择 adapter。
- 使用 switch/toggle 表示 enabled、inherit、progress card。
- 使用 path picker 或 combobox 管理 work_dir，禁止自由输入绕过 allowlist。
- 使用 code editor textarea 编辑 instructions，但默认高度克制，避免页面像配置文件编辑器。
- 使用 table 展示 skills，列出 name、scope、source path、enabled/disabled、shadowed 状态。
- 使用 diff-like preview 展示“全局值 vs 通道覆盖 vs 有效值”。
- 使用 warning callout 展示危险配置，但不要用弹窗打断普通编辑。

## 安全与权限

- Chat Gateway 默认只对 admin HTTP API 开放。
- `reply_mode=real_im` 需要显式 provider/target 和 send permission。
- request 中的 provider 配置只允许引用现有 provider id，不能传 secret。
- 所有响应只返回 masked/safe summary，不返回 token、secret_ref、完整 provider config。
- work_dir/add_dirs 必须经过 allowlist。
- adapter-specific 高风险参数需要单独提示；Codex 的 `approval_policy=never` 只允许在受控本地环境或外部 sandbox 中使用，生产默认应使用更保守策略。
- 图片和附件落盘需遵循现有 IM 限制，设置大小上限和 TTL。

## 测试方案

### 单元测试

- `chat_gateway_request_normalizes_im_context`：验证 HTTP 请求转换为 `NormalizedInboundMessage`。
- `chat_gateway_rejects_unallowed_work_dir`：验证未授权工程目录被拒绝。
- `external_cli_adapter_parser_maps_progress_events`：验证 adapter 私有事件转为 `AgentTurnProgressEvent`；Codex JSONL 是首个覆盖样例。
- `external_cli_adapter_capabilities_drive_config_schema`：验证 adapter 能力声明会决定 WebUI/API 可配置字段，未声明能力不会被错误下发。
- `external_cli_runtime_accepts_manifest_adapter`：验证简单 manifest adapter 能构造 `CommandSpec` 并复用 run dir / artifact / event pipeline。
- `chat_gateway_real_im_requires_permission`：验证默认不会发送到真实 IM。
- `agent_effective_config_marks_inherited_and_overridden_fields`：验证全局默认和通道覆盖合并后能标记字段来源。
- `agent_effective_config_rejects_channel_work_dir_expansion`：验证通道配置不能扩大到全局未允许目录。

### E2E 测试

- `im_gateway_external_cli_chat_gateway_basic`：启动真实 Bifrost，配置 mock external CLI adapter，调用 `/chat`，断言 final response 和 artifacts。
- `im_gateway_external_cli_chat_gateway_stream`：调用 `/chat/stream`，断言收到 run_started、tool_started、tool_finished、run_finished。
- `im_gateway_external_cli_chat_gateway_skill_context`：构造 repo/global/system skill，断言 prompt envelope 包含 metadata 和路径。
- `im_gateway_external_cli_chat_gateway_stop`：启动长运行 mock external CLI，调用 stop，断言进程结束、状态为 stopped。
- `im_gateway_codex_adapter_contract`：用 Codex adapter 覆盖 `codex exec --json`、`--cd`、`--add-dir`、`--output-last-message` 的命令构造和 final response 提取。
- `im_gateway_agent_config_webui_flow`：浏览器打开 Settings -> IM Gateway，配置 Agent Defaults、通道覆盖、Preview Effective Config 和 Run Test Message，断言最终快照、事件时间线和 run detail 可见。
- `im_gateway_agent_config_webui_theme`：在亮色/暗色主题下检查 Agent Defaults、通道 Agent tab、Chat Gateway 测试抽屉和 Runs 详情没有不可读文本、重叠或危险配置提示丢失。

### 真实场景测试

实现时新增 `human_tests/im-gateway-external-cli-chat-gateway.md`，至少覆盖：

- 通过 HTTP Chat Gateway 触发外部 CLI Agent，不依赖真实 IM 入站。
- Codex adapter 作为首个内置 adapter 可以完成同一流程。
- 同步响应包含 final response、run_id、artifacts。
- 流式响应可看到进度事件。
- 非 allowlist work_dir 被拒绝。
- 默认不向真实 IM 发送消息。
- 显式 `reply_mode=real_im` 在具备权限时可发送到测试 target。
- WebUI 中配置全局默认、单通道覆盖，预览有效配置，确认继承/覆盖来源清晰。
- WebUI 中使用 Chat Gateway 测试抽屉发起测试，查看流式时间线、progress card 预览、final response 和 Runs 详情。
- WebUI 亮色和暗色主题下完成同一配置/测试流程。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核入口边界：Chat Gateway 是否只替代入站事件，不复制 runtime。
- Review API schema、work_dir allowlist、reply_mode 安全边界。
- 使用 mock external CLI executable 跑单元和 E2E，并单独覆盖 Codex adapter contract。
- 修复 parser、stream 或权限遗漏。

### 第 2 轮

- 复查第 1 轮修复后的 diff。
- 复跑 `/chat`、`/chat/stream`、stop、skill context、work_dir 拒绝路径。
- 检查 human_tests 文档和 readme 索引。
- 如仍发现协议、安全或可测性缺口，追加第 3 轮。

## 当前实施进展

截至本方案迁入 `codex/im-external-cli-agent` worktree 后，已先落 V0 主链路：

- 新增 `im_gateway::external_cli`，提供 `ExternalCliRuntime`、Codex 默认 adapter 命令构造、通用 adapter args、JSONL progress event 归一化、run artifacts 写盘和 run detail 读取；已按本机真实 Codex CLI `0.130.0` 去掉旧 `--ask-for-approval` 参数映射。
- 新增 `POST /api/im-gateway/chat`，可以直接通过 HTTP 触发外部 CLI Agent，测试时不依赖真实 IM 入站消息。
- 新增 `GET /api/im-gateway/chat/runs/:run_id`，用于读取 runtime snapshot、stdout/stderr、normalized events 和 final response。
- 新增 `human_tests/im-gateway-external-cli-chat-gateway.md` 并同步 `human_tests/readme.md` 索引。
- 新增 `im_gateway_external_cli_agent.json` 配置存储，支持 `defaultRunnerId + runners{}` 的多 CLI runner 注册表；Codex 只是默认 runner，后续 Claude/Gemini/Trae/自定义 CLI 通过新增 runner 接入。
- 新增 Chat Gateway 配置 API：`GET/PATCH /chat/config` 保存完整 runner registry，`/chat/config/channels/:provider_id` 只保存 IM 通道的 `runnerId/enabled/deliveryMode` 覆盖。Chat Gateway 的 `/chat` 和 `/chat/stream` 会始终合并默认 runner；带 providerId 时再叠加 provider/channel 语义。
- 新增 `POST /api/im-gateway/chat/stream` NDJSON 测试入口，用于 WebUI 测试抽屉和接口级测试。
- 新增 `POST /api/im-gateway/chat/runs/:run_id/stop`，写 stop marker，并对 active CLI process 发送终止信号；Unix 下只在确认子进程拥有独立 process group 时才终止同组进程，避免 pid 0 或父进程组误伤；run 收敛阶段会优先识别 stop marker，即使 shell 迟到输出也固定返回 `status:"stopped"`。
- 新增 `ExternalCliAgentChat` route action，真实 IM 入站命中 route 后可走 External CLI runtime，并按 delivery mode 发送开始/最终回复。
- WebUI 新增 AI -> Agent -> Runners section，支持 runner 列表、新建/编辑弹窗、通道 runner 覆盖、effective preview 和测试运行。Runners 属于 Agent 能力，不放在 IM Gateway 子导航中。
- Agent 全局配置新增 `runner`，用于选择 IM 默认消息进入内置 Bifrost Agent 还是某个自定义 runner；IM Provider 的 `agent_config.runner` 可覆盖全局默认。Provider 选择自定义 runner 后，即使没有显式 `ExternalCliAgentChat` route，默认 Agent/AgentChat 入站消息也会调度到对应 runner runtime。
- `AgentConfig.runner` 与 `ImProviderAgentConfig.runner` 使用统一 runner 语义：`bifrost_agent` 表示内置 Agent，自定义 runner 直接保存 runner ID（如 `codex`、`abc`），避免把所有 CLI 能力压扁为单个 `external_cli` 枚举值。
- 已覆盖单元测试：progress parser、真实 Codex JSONL parser、Codex adapter contract、mock CLI runtime artifact pipeline、effective config 来源标记、stop 后迟到 stdout 不污染最终状态。
- 已完成真实 HTTP smoke：global/channel config、effective preview、`/chat`、run detail、`/chat/stream`、work_dir allowlist、`/stop`、`ExternalCliAgentChat` route 保存。
- 已完成真实 Codex CLI smoke：直接调用 `/opt/homebrew/bin/codex exec --json` 成功；Chat Gateway `/chat` 和 `/chat/stream` 使用真实 Codex adapter 成功返回最终答案，并落盘真实 Codex stdout JSONL、last message、normalized events。
- 已完成 WebUI 验证：Playwright 打开 `/_bifrost/ai?agentSection=runners`，亮色/暗色 `colorScheme` 下 Runners 管理页可见。
- WebUI 在 AI -> Agent -> General 暴露 Default Runner 选择；AI -> Agent -> Runners 只负责管理 runner 实体，不再提供第二个默认 runner 入口；IM Provider 编辑弹窗暴露 Agent Runner 覆盖项，支持继承全局默认、Bifrost Agent、任意自定义 runner ID 三类状态。
- Codex adapter 在运行时把解析后的工作目录显式映射成 `--cd <resolved_work_dir>`；即使 runner 自定义了 `args:["exec","--json","-"]`，runtime 仍会补齐 `--cd` 与 `--output-last-message <run>/last_message.md`，避免 Codex Desktop session 归属到 Bifrost 服务启动目录或 API final response 退化成 `turn completed`。
- 已完成真实 Codex CLI 工作目录验证：Provider runner 使用 `~/work/github/bifrost/crates/bifrost-admin` 返回 `WORKDIR_CHECK:~/work/github/bifrost/crates/bifrost-admin`，无 providerId 的 Chat Gateway 降级到全局 Agent `work_dir=~/work/github/bifrost` 返回 `GLOBAL_WORKDIR_CHECK:~/work/github/bifrost`；两个 run 的 `runtime_snapshot.json` 都包含显式 `--cd`。

V0 尚未落地的设计项：

- `/chat/stream` 当前为 run 级 NDJSON 输出，已经能返回 progress events，但还不是 stdout 行级实时转发；长运行进程实时 coalescing 可作为下一步增强。
- `/runs/:run_id/stop` 已能终止 active process 并收敛为 stopped；adapter 原生协作取消协议可在各 adapter manifest 中进一步声明。
- Bifrost 自带工具集合 V0 通过 prompt 注入本地 `bifrost` CLI 使用约束；完整 MCP server 形式仍待补。
- WebUI 已完成配置、测试入口和亮暗主题基础浏览器验证；更细的交互可用性走后续 UI 回归扩展。
- Working directory 不属于自定义 runner。runner 运行时优先继承 IM Provider 的 Agent Working Directory，再降级到全局 Agent Working Directory；runner 自身只描述 CLI 可执行文件、参数、指令、skill 和 delivery 默认值。
- `/` 命令兼容性当前分层处理：内置 Bifrost Agent runner 完整支持忙碌时默认 guide、`/g` 显式 guide、`/q` 排队、`/rq` 移除、`/status` 和 `/stop`；自定义 runner 链路复用同一 busy-session 入口，但普通追加消息默认 queue，`/g` 会明确降级为 queue，`/stop` 会映射到 active runner 进程，并在确认子进程组隔离后终止其进程组。
- Codex CLI 能力边界：当前使用的 `codex exec --json ... -` 只在启动时读取 prompt/stdin，不支持运行中追加 guide；`codex exec resume <thread_id> ... -` 支持当前 run 完成后的下一轮接续。因此 Codex Runner busy 期间仍按 queue 处理，队列 drain 时继承上一轮 JSONL 解析出的 `threadId`，用 resume 兼容“追加消息”的会话连续性。

## 待讨论问题

1. Chat Gateway 默认是否只允许 admin token，还是允许绑定某个 IM provider 的 owner 身份模拟调用？
2. 流式协议优先 NDJSON 还是 SSE？NDJSON 更贴近多数 CLI 的 JSONL 输出，SSE 更适合浏览器 UI。
3. `reply_mode=real_im` 是否放在 V1，还是先只做测试流，避免误发真实 IM？
4. 外部 CLI session 是否默认每 turn 独立进程，还是由 adapter 声明 resume 能力后保持更强上下文连续性？Codex adapter 可选 `codex exec resume`。
5. Bifrost 工具集合 V1 是否只走 CLI + skill，还是首版就补 `bifrost mcp-server`？
6. WebUI 是否在 V1 就支持 target/chat 级 Agent 覆盖，还是先只做到 global + provider/channel 两层？
7. Chat Gateway 测试抽屉的流式协议在前端内部使用 NDJSON 还是 SSE？
