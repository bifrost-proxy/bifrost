# Agent Model Config Preflight

## 功能模块说明

内置 Bifrost Agent 在开始普通 Chat 或需要模型参与的 `/compact` 前，必须先检查模型配置是否完整。缺少模型名、Provider `base_url`、AK、或鉴权 Header 所依赖的环境变量时，Agent 不应继续请求模型服务，也不应把模型网关返回的 401/403 原始错误直接暴露给用户。

用户可感知目标：

- Web/IM/Schedule 等内置 Agent 入口复用同一套配置预检。
- 缺配置时返回一条正常 assistant 消息，提示缺少哪些配置，以及去 `Settings → Agent → Model Configuration` 或 `config.toml` 配置。
- 使用 `$ENV_NAME` 或 `env_key` / `env_http_headers` 时，明确提示变量必须存在于启动 Bifrost 的 shell 或服务环境，配置后需要重启。
- 自定义本地 Provider 可以显式不配置 AK；只有设置了 `env_key`、`api_key`、或鉴权类 env header 时才要求对应值非空。

## 实现逻辑

`crates/agent/src/config_preflight.rs` 新增 `AgentConfig::preflight_model_config()`：

- 解析当前 `model_provider` 的有效 Provider 配置。
- 对内置 Provider 保留字段级 fallback；对未知自定义 Provider 不再隐式 fallback 到 `OPENAI_API_KEY`，避免本地无鉴权 Provider 被误判。
- 检查 `model`、`base_url`、`api_key`、`env_key`。
- 检查 `env_http_headers` 中的鉴权 Header：`Authorization`、`api-key`、`x-api-key`。
- 不要求可选 Header，例如 `X-TT-LOGID`、`OpenAI-Organization`、`OpenAI-Project`。

`crates/agent/src/session/turn_loop.rs`：

- 普通 Chat 在写入用户消息和发起模型请求前执行预检。
- `/compact` 属于模型依赖命令，也执行同样预检。
- 缺配置时写入 user + assistant 历史，记录 `TurnCompleted`，返回正常 `TurnResult`。

`crates/agent/src/client.rs`：

- 在 `chat_completion_with_schema` 入口再次执行预检，防止后续新增路径绕过 turn loop 直接请求模型。

## 依赖项

- 复用现有 `AgentConfig` / `ModelProviderConfig` / `AgentClient` / `AgentSession`。
- 无新增外部 crate。

## 测试方案

### 单元测试

- `test_preflight_model_config_reports_missing_model`：缺少模型名时返回友好配置提示。
- `test_preflight_model_config_reports_missing_env_key`：Provider `env_key` 缺失时返回具体环境变量名。
- `test_preflight_model_config_requires_auth_header_env_only`：鉴权 Header env 缺失会提示，可选 Header 不提示。
- `test_preflight_model_config_allows_local_provider_without_key`：自定义本地 Provider 无 AK 时允许通过。
- `test_preflight_model_config_accepts_api_key_env_reference`：`api_key = "$ENV_NAME"` 且环境变量存在时通过。

### E2E 测试

- `test_missing_model_config_returns_guidance_without_model_request`：启动 mock 模型服务，配置缺 AK，执行一次 Agent turn，断言模型服务请求数为 0，turn 正常返回配置指引并写入历史。
- `chat_completion_rejects_empty_api_key_before_http_request`：直接调用 client，断言空 `api_key` 在 HTTP 前被拒绝，不出现连接错误。

### 真实场景测试

对应 `human_tests/agent-chat-config-preflight.md`：

- 缺 AK 环境变量时普通 Chat 返回友好提示。
- 缺模型名时返回友好提示。
- 空 `api_key` 不再继续 HTTP 请求。
- 自定义无鉴权 Provider 不被误拦截。

## Review/Fix/Test 闭环方案

第 1 轮：

- 复核用户目标和 diff，重点检查是否仍有 raw 401 泄漏路径。
- 运行配置层、turn 层、client 层 targeted tests。
- 修复测试或代码发现的问题。

第 2 轮：

- 复查自定义 Provider fallback、可选 Header、历史写入、`/compact` 路径。
- 复跑 targeted tests、fmt、clippy、相关 crate test。
- 确认 design/human_tests/readme 同步。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo test -p bifrost-agent preflight_model_config -- --nocapture`
- `cargo test -p bifrost-agent test_missing_model_config_returns_guidance_without_model_request -- --nocapture`
- `cargo test -p bifrost-agent chat_completion_rejects_empty_api_key_before_http_request -- --nocapture`
- `cargo test -p bifrost-agent --lib`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`

## 文档更新要求

- 新增本设计文档。
- 新增 `human_tests/agent-chat-config-preflight.md`。
- 更新 `human_tests/readme.md` 索引。
