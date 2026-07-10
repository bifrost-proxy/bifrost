# IM /help 命令测试

## 功能模块说明

当用户通过 IM（飞书消息）发送 `/help` 命令时，Agent 应返回所有可用命令的帮助信息，而非"未知命令"。帮助信息需要区分内置 Agent 命令和 IM 通道专属控制命令，避免 `/cwd`、`/runner`、排队和引导能力只存在于代码里但用户不可发现。

Provider 上线通知也必须在原有在线提示后追加同一套帮助命令，并按当前通道绑定的 Runner 类型裁剪命令范围。所有 Runner 都支持 IM 通道控制命令；只有内置 Bifrost Agent 支持 memory/goal/context/skill 等内置 Agent 命令；Codex、Traex、Claude Code 等非 ChatGPT Web Runner 还展示 Guide 与模型/effort/停止/排队控制，不能把不支持的内置 Agent memory 管理命令推给用户。

## 前置条件

1. 启动 Bifrost 服务：
```bash
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
```
2. 确认服务正常运行，WebUI 可访问 `http://localhost:8800/_bifrost/`

## 测试用例

### TC-IH-01: /help 命令返回帮助信息

**操作步骤**：
1. 通过 Agent API 发送 `/help` 消息（或通过 IM 发送）
2. 检查响应内容

**预期结果**：
- 响应包含"可用命令:"标题
- 响应包含"内置命令:"分类
- 响应列出所有内置命令：/help、/clear、/reset、/undo、/compact、/status、/stop、/resume、/remember、/memories、/forget、/goal、/skill
- IM 响应额外包含"IM 通道命令:"分类
- IM 通道命令列出 `/cwd <绝对路径>`、`/runner [Runner]`、`/q <消息>`、`/rq <序号>`、`/g <引导内容>`
- 每个命令附带中文描述说明
- 响应末尾包含"提示: 直接输入文本即可与 AI 对话。"

### TC-IH-02: /help 不再返回"未知命令"

**操作步骤**：
1. 通过 Agent API 发送 `/help` 消息
2. 检查响应不包含"未知命令"

**预期结果**：
- 响应中不包含"未知命令"字样
- 响应为格式化的帮助文本

### TC-IH-03: 真正的未知命令仍返回错误

**操作步骤**：
1. 通过 Agent API 发送 `/foobar` 消息
2. 检查响应

**预期结果**：
- 响应为"未知命令: /foobar"

### TC-IH-04: IM /help 暴露 IM 专属控制命令

**操作步骤**：
1. 通过 IM 通道发送 `/help` 消息。
2. 检查响应中的 IM 通道命令区域。

**预期结果**：
- 响应包含"IM 通道命令:"。
- 响应包含 `/cwd <绝对路径>`，并说明路径必须存在且是目录。
- 响应说明运行中 `/cwd` 会排队到当前任务结束后执行。
- 响应包含 `/runner [Runner]`，并说明可查看或切换当前 IM 通道绑定的 Runner。
- 响应包含 `/q <消息>`、`/rq <序号>`、`/g <引导内容>`。
- `/cwd` 和 `/runner` 不作为 WebUI/API Agent slash command 注册；WebUI/API 普通 `/help` 不需要展示这些 IM 专属命令。

### TC-IH-05: Provider 上线通知自动追加帮助命令

**操作步骤**：
1. 使用临时数据目录启动 Bifrost，并创建一个 enabled Feishu 或 Weixin Provider。
2. 触发 Provider connect 或重启服务，使 Provider 发送上线通知。
3. 查看发送给 owner 的上线通知消息内容，或查询 message log 中 `trigger=online` 的 `content_preview`。

**预期结果**：
- 上线通知仍包含原有在线提示：Provider、Device、Workspace、Runner Type、Runner ID、Model、Reasoning Effort、Bound Session、Completed User Turns、Status。
- 在线提示后追加 `可用命令:` 帮助区。
- 帮助区包含 `IM 通道命令（所有 Runner）:`，并列出 `/cwd`、`/runner`、`/q`、`/rq`。
- 用户在 IM 通道连接建立时无需主动发送 `/help`，即可看到可用命令。
- 原有上线通知字段不被帮助文案覆盖或截断。

### TC-IH-06: 内置 Bifrost Agent 上线帮助展示内置 Agent 命令和 `/g`

**操作步骤**：
1. 创建或使用绑定内置 `bifrost_agent` 的 Provider。
2. 触发 Provider connect 或重启服务。
3. 检查上线通知帮助区。

**预期结果**：
- 帮助区包含 `Bifrost Agent 命令:`。
- 展示 `/clear`、`/compact`、`/forget`、`/goal`、`/help`、`/memories`、`/remember`、`/reset`、`/resume`、`/skill`、`/status`、`/stop`、`/undo` 等内置 Agent 命令。
- `IM 通道命令（所有 Runner）:` 中展示 `/g <引导内容>`，因为内置 Agent 支持运行中引导注入。
- 命令描述使用中文，且提示 `直接输入文本即可与 AI 对话`。

### TC-IH-07: 外部 Runner 上线帮助不展示内置 Agent memory/goal/skill 命令

**操作步骤**：
1. 创建或使用绑定外部 Runner 的 Provider，例如 `chatgpt_web`、`codex`、`traex` 或 `Claude-Code`。
2. 触发 Provider connect 或重启服务。
3. 检查上线通知帮助区。

**预期结果**：
- 帮助区包含 `IM 通道命令（所有 Runner）:`，展示 `/cwd`、`/runner`、`/q`、`/rq`。
- 帮助区不展示 `/remember`、`/memories`、`/forget`、`/goal`、`/skill`、`/compact` 等仅内置 Agent 支持的命令。
- Codex/Traex/Claude Code 等非 ChatGPT Web Runner 展示 `/g <引导内容>`，并提示普通后续消息默认按 Guide 处理、使用 `/q` 才排队；ChatGPT Web 不展示 `/g`。
- Codex/Traex/Claude Code 这类 Runner 展示它们支持的 `/models`、`/model`、`/effort` 或对应 runner-specific 控制命令。

### TC-IH-08: Runner-aware `/help` 与上线帮助保持一致

**操作步骤**：
1. 在内置 Agent Provider 上通过 IM 发送 `/help`。
2. 在外部 Runner Provider 上通过 IM 发送 `/help`。
3. 对比各自 `/help` 响应与该 Provider 上线通知中的帮助区。

**预期结果**：
- 同一 Runner 类型下，主动 `/help` 与上线通知帮助区的命令分组一致。
- 内置 Agent 展示内置命令和 IM 通道命令。
- 外部 Runner 只展示所有 Runner 通用命令和对应 Runner 支持的命令。
- 未知命令 `/foobar` 仍返回 `未知命令: /foobar`，不会被启动帮助逻辑吞掉。

## 执行记录

- 2026-06-08：执行 focused 单测 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin im_help_includes_im_only_commands_without_dropping_builtins --lib -- --nocapture`，验证 IM `/help` 在保留内置命令说明的同时追加 `/cwd`、`/runner`、`/q`、`/rq`、`/g` 说明；执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin im_cwd_command --lib -- --nocapture`，复跑 `/cwd` 解析、非法路径和 Provider work_dir 持久化回归。
- 2026-07-08：执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin online_notification_message_ --lib -- --nocapture`，验证上线通知在原有 Provider/Device/Workspace/Runner/Session 状态后追加 runner-aware 帮助；内置 Bifrost Agent 通知包含 `IM 通道命令（所有 Runner）:`、`Bifrost Agent 命令:`、`/remember <text>` 与 `/g <引导内容>`；外部 `chatgpt_web` runner 通知不包含 `/remember`、`/goal` 或 `/g <引导内容>`；Claude Code runner 通知包含 `Claude Code Runner 命令:`、`/models`、`/efforts`，且不暴露内置 Agent memory 命令。

## 清理步骤

1. 停止 Bifrost 服务
2. 删除临时数据目录：`rm -rf ./.bifrost-test`
