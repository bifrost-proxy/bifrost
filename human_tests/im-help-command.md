# IM /help 命令测试

## 功能模块说明

当用户通过 IM（飞书消息）发送 `/help` 命令时，Agent 应返回所有可用命令的帮助信息，而非"未知命令"。

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
- 响应列出所有内置命令：/help、/clear、/reset、/undo、/compact、/status、/resume、/remember、/memories、/forget、/skill
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

## 清理步骤

1. 停止 Bifrost 服务
2. 删除临时数据目录：`rm -rf ./.bifrost-test`
