# IM /help 命令测试

## 功能模块说明

当用户通过 IM（飞书消息）发送 `/help` 命令时，Agent 应返回所有可用命令的帮助信息，而非"未知命令"。帮助信息需要区分内置 Agent 命令和 IM 通道专属控制命令，避免 `/cwd`、`/runner`、排队和引导能力只存在于代码里但用户不可发现。

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

## 执行记录

- 2026-06-08：执行 focused 单测 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin im_help_includes_im_only_commands_without_dropping_builtins --lib -- --nocapture`，验证 IM `/help` 在保留内置命令说明的同时追加 `/cwd`、`/runner`、`/q`、`/rq`、`/g` 说明；执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin im_cwd_command --lib -- --nocapture`，复跑 `/cwd` 解析、非法路径和 Provider work_dir 持久化回归。

## 清理步骤

1. 停止 Bifrost 服务
2. 删除临时数据目录：`rm -rf ./.bifrost-test`
