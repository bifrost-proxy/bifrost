# Agent Chat Config Preflight 真实场景测试用例

## 功能模块说明

验证内置 Bifrost Agent 在普通 Chat 和模型依赖路径执行前，会先检查模型配置完整性。缺少模型、AK 或鉴权环境变量时，用户应看到可操作的配置指引，而不是模型网关原始 401/403 错误。

## 前置条件

- 在仓库根目录执行命令。
- 每条命令前执行 `source ~/.zshrc`。
- 使用测试内自定义 Provider 和唯一环境变量名，不依赖本机真实 `MODELHUB_AK`。
- 不需要启动真实 Bifrost 服务；本轮通过 Agent turn 和 client 边界测试验证 Web/IM 共享的内置 Agent 后端路径。

## 测试用例列表

### TC-ACCP-01：缺 AK 环境变量时 Chat 返回配置指引

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   cargo test -p bifrost-agent test_missing_model_config_returns_guidance_without_model_request -- --nocapture
   ```
2. 检查测试输出通过。

预期结果：

- Agent turn 返回正常 `TurnResult`，响应包含 `内置 Agent 模型配置不完整`。
- 响应包含缺失环境变量 `BIFROST_AGENT_TEST_TURN_MISSING_AK`。
- 响应包含 `Settings → Agent → Model Configuration`。
- mock 模型服务请求数为 0，证明没有继续请求模型网关。

### TC-ACCP-02：缺模型名时返回配置指引

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   cargo test -p bifrost-agent test_preflight_model_config_reports_missing_model -- --nocapture
   ```
2. 检查测试输出通过。

预期结果：

- 配置预检失败信息包含 `未配置模型名称`。
- 信息包含 Web UI 配置路径和 `config.toml`。

### TC-ACCP-03：空 api_key 在 HTTP 请求前被拦截

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   cargo test -p bifrost-agent chat_completion_rejects_empty_api_key_before_http_request -- --nocapture
   ```
2. 检查测试输出通过。

预期结果：

- client 返回配置预检错误，包含 `api_key`。
- 错误不包含 `Connection refused`，证明没有继续发起 HTTP 请求。

### TC-ACCP-04：自定义无鉴权 Provider 不被误拦截

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   cargo test -p bifrost-agent test_preflight_model_config_allows_local_provider_without_key -- --nocapture
   ```
2. 检查测试输出通过。

预期结果：

- 自定义 Provider 显式配置 `base_url` 且未配置 `env_key` / `api_key` / 鉴权 Header 时，预检通过。
- 不会因为未知 Provider 自动要求 `OPENAI_API_KEY`。

## 清理步骤

- 测试使用的唯一环境变量会在测试内清理。
- 未启动 Bifrost 服务，无需停止进程。

## 执行记录

| 日期 | 用例 | 命令 | 结果 |
| --- | --- | --- | --- |
| 2026-06-04 | TC-ACCP-01 | `source ~/.zshrc; cargo test -p bifrost-agent test_missing_model_config_returns_guidance_without_model_request -- --nocapture` | PASS：Agent turn 返回正常配置指引，包含缺失变量 `BIFROST_AGENT_TEST_TURN_MISSING_AK` 与 `Settings → Agent → Model Configuration`，mock 模型服务请求数为 0 |
| 2026-06-04 | TC-ACCP-02 | `source ~/.zshrc; cargo test -p bifrost-agent test_preflight_model_config_reports_missing_model -- --nocapture` | PASS：缺模型名时提示 `未配置模型名称`，并包含 Web UI 配置路径和 `config.toml` |
| 2026-06-04 | TC-ACCP-03 | `source ~/.zshrc; cargo test -p bifrost-agent chat_completion_rejects_empty_api_key_before_http_request -- --nocapture` | PASS：空 `api_key` 在 client HTTP 前返回配置预检错误，不出现 `Connection refused` |
| 2026-06-04 | TC-ACCP-04 | `source ~/.zshrc; cargo test -p bifrost-agent test_preflight_model_config_allows_local_provider_without_key -- --nocapture` | PASS：未知自定义 Provider 不再隐式要求 `OPENAI_API_KEY`，无鉴权本地 Provider 预检通过 |
