# Agent Model Config Preflight

## 背景

Bifrost 内置 Agent（Web/Admin `/api/agent/chat/stream`、IM Gateway `/_bifrost/api/im-gateway/agent/chat`、内置 Schedule、`/compact` 等）在开始每个模型请求前，都需要一份完整的 `AgentConfig`：模型名、Provider `base_url`、AK 或环境变量鉴权、必要的鉴权 Header。旧路径会把不完整配置直接送到 `AgentClient::chat_completion_with_schema()`，最终由模型网关返回 401/403 甚至连接错误，模型层拿不到语义提示，用户看到的是 raw 网关错误：`"401: {\"error\":\"invalid api key\"}"`、`Connection refused`、`getaddrinfo ENOTFOUND`。

`config_preflight.rs::AgentConfig::preflight_model_config()` 是集中的配置预检入口，覆盖两个调用点：

- `crates/agent/src/session/turn_loop.rs::run_turn_with_mcp_multimodal()` 的普通 Chat 分支与 `/compact` 分支（当前 `turn_loop.rs:1161` 与 `turn_loop.rs:1522`）。
- `crates/agent/src/client.rs::chat_completion_with_schema()` 入口（当前 `client.rs:132`），防止未来新增路径绕过 turn loop 直接命中模型。

用户看到的效果：缺配置时 Agent 返回一条正常 assistant 消息，指出缺哪些字段、去哪配置、如果依赖环境变量必须重启服务；而不是把网关 401/403 或连接错误当作模型答复原样吐出。

## 用户目标验证清单

### 必须实现

- Web/IM/Schedule 等所有内置 Agent 入口复用同一份 `preflight_model_config()`。
- 缺配置时 Agent 返回一条正常 assistant 消息，提示缺少哪些字段（`model`、`base_url`、`api_key`、`env_key`、鉴权 Header 的 env 变量），并指向 `Settings → Agent → Model Configuration` 或 `~/.bifrost/agent/config.toml`。
- 使用 `$ENV_NAME`、`env_key`、`env_http_headers` 时，明确提示变量必须存在于启动 Bifrost 的 shell / launchd / 服务环境，配置后需要重启 Bifrost 进程。
- 自定义本地 Provider（例如 `http://127.0.0.1:8080/v1/chat/completions`）可以显式不配置 AK；只有当设置了 `env_key`、`api_key` 或 `Authorization` / `api-key` / `x-api-key` 之类 env header 时才要求对应值非空。
- 内置 Provider（`aidp_crawl` 等）若用户已经在 `http_headers` 里手动配置了非空静态鉴权 Header，视为鉴权已满足，不再因为 builtin fallback 出来的 `env_key` 未设置而误拦截。
- `/compact` 属于模型依赖命令，走同样预检；不会因“已在 slash 命令内”就跳过。
- `AgentClient::chat_completion_with_schema()` 入口再执行一次预检，抵御未来新入口直接构造 client 请求模型。
- 预检失败时写入完整 user + assistant 历史（`persistence::record_user_message`、`record_assistant_response`），并生成 `TurnCompleted` 事件，保证 SSE / IM 卡片流程收敛而不是挂起。

### 必须不破坏

- 不修改 `AgentConfig` / `ModelProviderConfig` 字段。
- 不修改 `chat_completion_with_schema()` 现有签名。
- 已配置齐全时预检不引入额外的 HTTP 请求或环境变量读取延迟。
- 不影响 skill / MCP / Compact 等非 turn_loop 内部的独立配置逻辑。
- 不阻止 `/status`、`/version`、`/reset`、`/clear` 等本地纯控制命令。

### 必须真实验证

- 单元测试覆盖：缺 model、缺 env_key、鉴权 Header env 缺失（可选 Header 不提示）、本地 Provider 无 AK 通过、`$ENV_NAME` 正常引用通过、静态鉴权 Header 通过。
- E2E 覆盖：mock 模型服务启动前配置缺 AK，Agent 一轮请求后 mock 服务请求计数为 0，SSE 返回配置指引；空 `api_key` 在 HTTP 前被拒。
- human_tests 覆盖：真实 Web/IM/Admin 缺配置流程可视、`/compact` 走同样预检、Provider 切换后重启前后行为一致。

### 必须交付

- 更新 `crates/agent/src/config_preflight.rs`、`crates/agent/src/session/turn_loop.rs`、`crates/agent/src/client.rs`。
- 更新 `human_tests/agent-chat-config-preflight.md` 与 `human_tests/readme.md` 索引。
- 完成至少两轮 Review/Fix/Test 闭环。

## 产品语义

### 什么算“配置齐全”

用户视角把 provider 分成三档：

1. **内置 Provider**（`aidp_crawl` 等，由 `get_builtin_provider()` 提供 fallback）：默认已经有 `base_url`、`env_key`、鉴权 Header 结构；用户只需要设置对应环境变量或在 `http_headers` 中填静态 token。
2. **自定义远端 Provider**：`base_url` 必填；`api_key` 或 `env_key` 或 `env_http_headers` 三选一给到鉴权手段。
3. **本地无鉴权 Provider**（`http://127.0.0.1:*`、`http://localhost:*`）：允许 `env_key = None` + `api_key = None` + 无鉴权 header；预检直接通过。

### 什么算“配置不齐”

`preflight_model_config()` 逐项检查并按顺序拼装 `issues`：

- `model` 为空或空白：`未配置模型名称 model`。
- `base_url` 为空：`模型 Provider {id} 缺少 base_url，无法知道请求发往哪里`。
- `api_key = ""`：`Provider {label} 的 api_key 为空`。
- `api_key = "$"` 或 `api_key = "$ENV_NAME"` 但环境变量缺失/空：`环境变量 ENV_NAME 未设置或为空，Provider {label} 无法读取 AK`。
- `env_key = ""` 或该 env 变量缺失/空：`环境变量 {env_key} 未设置或为空，Provider {label} 缺少 AK`。
- `env_http_headers` 中 `Authorization` / `api-key` / `x-api-key` 三个鉴权 header 引用的 env 变量缺失/空。
- 已在 `http_headers` 里显式配置了非空静态鉴权 Header 时，跳过 env_key + env_http_headers 的强制检查（避免用户手动填 token 却被 builtin fallback 拦截）。
- 可选 Header（`X-TT-LOGID`、`OpenAI-Organization`、`OpenAI-Project` 等）不算鉴权，不参与拦截。

### 预检失败时的回复

Agent 返回一条正常 assistant 消息，格式由 `format_model_config_guidance()` 生成：

```text
内置 Agent 模型配置不完整，暂未开始执行。

缺少配置：
- 未配置模型名称 model。
- 环境变量 MODELHUB_AK 未设置或为空，Provider Model Hub 缺少 AK。

请到 Web UI 的 Settings → Agent → Model Configuration 配置 Model、Model Provider 和 API Key；如果使用环境变量，请在启动 Bifrost 的 shell 或服务环境中设置对应变量后重启 Bifrost。

也可以直接编辑 `~/.bifrost/agent/config.toml`，当前 Provider: `aidp_crawl`。API Key 支持直接填写，或填写 `$ENV_NAME` 从环境变量读取。
```

该消息落 assistant 历史、`TurnCompleted` 事件，SSE / IM 卡片收敛。不能只 log warn 就静默 return。

## 技术细节

### 关键 API 与文件

- `crates/agent/src/config_preflight.rs`
  - `impl AgentConfig { pub fn preflight_model_config(&self) -> Result<(), String>; }`（`config_preflight.rs:8`）
  - `pub(crate) fn resolve_model_provider_config(&self) -> (String, ModelProviderConfig);`（`config_preflight.rs:62`）
  - 内部 helper：`missing_api_key_issue()`、`missing_auth_header_issues()`、`has_non_empty_static_auth_header()`、`is_auth_header()`、`format_model_config_guidance()`、`empty_provider_fallback()`。
- `crates/agent/src/session/turn_loop.rs`
  - 普通 Chat 分支：`turn_loop.rs:1522` 处调用；配置缺失返回 `config_preflight_turn_result()`。
  - `/compact` 分支：`turn_loop.rs:1161` 处调用；配置缺失同样走 `config_preflight_turn_result()`，不进入 compaction pipeline。
- `crates/agent/src/client.rs`
  - `chat_completion_with_schema()` 入口 `client.rs:132` 处 `config.preflight_model_config()?;`，将 `Err(String)` 转成 client 层错误，避免绕过 turn loop 的路径直接请求模型。

### Provider 解析规则

`resolve_model_provider_config()` 决定用哪份 provider 做预检：

- `provider_id = self.model_provider.trim()`；空则默认 `"aidp_crawl"`。
- 若 `provider_id` 在 `builtin_provider_ids()` 中，取 `get_builtin_provider(id)` 作为 fallback；否则 fallback 用 `empty_provider_fallback()`（全 None），避免把未知自定义 Provider 隐式补上 `OPENAI_API_KEY` 等内置 env_key。
- 用户 `model_providers[id]` 存在时，字段级 `or(builtin)` 合并；用户显式设置的字段覆盖 builtin。

### 鉴权 Header 识别

`is_auth_header()` 大小写不敏感地匹配 `authorization`、`api-key`、`x-api-key`。其它 header（`X-TT-LOGID` 等）不参与鉴权判定，也不参与拦截。

### 静态鉴权 Header 短路

`has_non_empty_static_auth_header()` 遍历 `provider.http_headers`，任一鉴权 header 值 trim 非空即返回 true。命中后：

- 跳过 `api_key` 缺失检查（仍会执行 `api_key = ""` 与 `$ENV_NAME` 空引用的检查）。
- 跳过 `env_http_headers` 中鉴权 header 的 env 缺失检查。
- 不影响 `base_url` / `model` / `api_key = ""` 检查。

### 报错去重

`missing_api_key_issue()` 与 `missing_auth_header_issues()` 共享 `reported_env: HashSet<String>`：同一个环境变量只在提示中出现一次；避免 `env_key = OPENAI_API_KEY` 且 `Authorization = "Bearer $OPENAI_API_KEY"` 时报两条。

### 预检失败结果

`config_preflight_turn_result()`（`turn_loop.rs`）：

- 记录 `user_message` 与附带 `images`。
- 写入 assistant response = 预检文案。
- 写入 `TurnCompleted { stop_reason: "config_preflight" }`。
- 返回 `TurnResult { plan_steps: session.current_plan.clone(), ... }`，让 SSE / IM 卡片正常收敛。

## CLI / Web / Admin API

### CLI

- `bifrost agent status` / `/status` 输出 `Model: <model>`、`Provider: <provider_id>`、`Auth: env(FOO)` / `static header` / `none`；帮助用户快速判断预检是否会通过。
- 用户配置错误后，任何入口触发 Agent turn 都会看到统一预检提示；CLI 不新增独立 `agent preflight` 子命令，避免出现“通过 CLI 通过、通过 API 拦截”的分裂。

### Web UI

- `Settings → Agent → Model Configuration` 页面新增“配置检查通过 / 缺少字段”提示，直接调用同一后端 API：`GET /_bifrost/api/agent/config/preflight` 返回 `{ ok: bool, issues: string[] }`；页面存表单前也复用该结果。
- 若用户依赖环境变量，页面在“保存”按钮旁提示：`环境变量修改后需要重启 Bifrost 才生效`。

### Admin API

- `GET /_bifrost/api/agent/config/preflight`：调用 `AgentConfig::preflight_model_config()`，返回：

  ```json
  { "ok": true }
  { "ok": false, "provider": "aidp_crawl", "issues": ["..."], "message": "..." }
  ```

- `POST /_bifrost/api/agent/chat/stream` 与 `POST /_bifrost/api/im-gateway/agent/chat` 触发 turn loop，缺配置时 SSE 首帧即返回预检 assistant 消息，`TurnCompleted` 之后关闭流。
- `POST /_bifrost/api/agent/chat/completions`（内部封装）通过 `client.rs:132` 的预检被拒时返回 4xx JSON `{ code: "AGENT_MODEL_CONFIG_MISSING", message: <format_model_config_guidance>, provider }`，调用方不必再 parse 网关原始错误。

## Sync 边界

- `AgentConfig` 存储在本地 `~/.bifrost/agent/config.toml`，不参与远端 rule / group / value sync。
- 环境变量属于宿主机运行时，不同步；`env_key` / `env_http_headers` 只保存名字，不保存值。
- Admin API 预检结果不写入 sync 队列；每次调用都基于当前进程的 `AgentConfig` 与 `std::env`。
- 多设备场景不共享 AK；每台机器需要独立配置或 `$ENV_NAME` 引用。

## 实现切分

### Phase 1：预检 API 与 turn loop 拦截

- 新增 `config_preflight.rs::preflight_model_config()` 与 helper 函数。
- `turn_loop.rs::run_turn_with_mcp_multimodal()` 普通 Chat / `/compact` 分支调用预检，缺配置走 `config_preflight_turn_result()`。
- 单元测试覆盖 `preflight_model_config` 六个基础用例。

### Phase 2：Client 层兜底

- `client.rs::chat_completion_with_schema()` 入口再执行一次预检。
- 单元 / E2E 测试：直接构造 client 且空 `api_key`，断言 HTTP 前被拒。

### Phase 3：Admin API 与 Web UI

- 新增 `GET /_bifrost/api/agent/config/preflight` 与 Web `Settings → Agent → Model Configuration` 检查提示。
- SSE 首帧返回预检文案；`TurnCompleted` 收敛。
- Playwright 覆盖 Web 端“缺 AK 提示”与“修复后能正常发送”。

### Phase 4：human_tests、文档、readme

- 更新 `human_tests/agent-chat-config-preflight.md` 与 `human_tests/readme.md` 索引。
- 更新 `docs/`、`docs-en/` 中 Agent 配置章节，指向预检 API。

## 测试方案

### 单元测试（`crates/agent/src/config_preflight.rs::tests`）

- `test_preflight_model_config_reports_missing_model`
- `test_preflight_model_config_reports_missing_env_key`
- `test_preflight_model_config_requires_auth_header_env_only`
- `test_preflight_model_config_allows_local_provider_without_key`
- `test_preflight_model_config_accepts_api_key_env_reference`
- `test_preflight_model_config_accepts_static_auth_header_without_env_key`

补充：

- `test_preflight_model_config_reports_missing_base_url`
- `test_preflight_model_config_deduplicates_shared_env_variable`
- `test_preflight_model_config_ignores_optional_headers`（`X-TT-LOGID` 等不引起拦截）
- `test_preflight_model_config_reports_empty_env_reference`（`api_key = "$"` 被识别为空引用）

### E2E 测试

- `test_missing_model_config_returns_guidance_without_model_request`（mock 模型服务请求计数为 0）。
- `chat_completion_rejects_empty_api_key_before_http_request`（直接调用 client，断言空 `api_key` 在 HTTP 前拒绝，不出现连接错误）。
- `test_agent_config_preflight_admin_api`：`GET /_bifrost/api/agent/config/preflight` 在缺配置时返回 `{ ok: false, issues: [...] }`，配置齐全后 `{ ok: true }`。

### 真实场景测试

`human_tests/agent-chat-config-preflight.md`（当前 111 行）扩展覆盖：

- TC-AMCP-01：缺 `MODELHUB_AK` 环境变量时 Web/Admin `/api/agent/chat/stream` 返回预检指引，不请求模型。
- TC-AMCP-02：缺 `model` 字段时返回预检指引。
- TC-AMCP-03：直接调用 `chat_completion_with_schema` 且空 `api_key` 时不产生 HTTP 请求。
- TC-AMCP-04：自定义本地 Provider 无鉴权时正常放行，能发起 mock 请求。
- TC-AMCP-05：内置 Provider 已配静态 `Authorization` header 时不因 `env_key` 未设置被拦截。
- TC-AMCP-06：`/compact` slash 命令在缺配置时同样返回预检指引，不进入 compaction pipeline。
- TC-AMCP-07：IM Gateway `/_bifrost/api/im-gateway/agent/chat` 场景（feishu 卡片）返回预检消息作为 assistant 卡片文本。
- TC-AMCP-08：`GET /_bifrost/api/agent/config/preflight` 与实际 turn 拦截结果一致（同一份 `preflight_model_config` 输出）。

### Coverage 与项目校验

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-agent preflight_model_config -- --nocapture`
- `cargo test -p bifrost-agent test_missing_model_config_returns_guidance_without_model_request -- --nocapture`
- `cargo test -p bifrost-agent chat_completion_rejects_empty_api_key_before_http_request -- --nocapture`
- `cargo test -p bifrost-agent --lib`
- `cargo test -p bifrost-admin --lib config_preflight`
- `cargo test --workspace --all-features`
- `rust-project-validate`

本机若维持 no-local-coverage 约定，可跳过 `make coverage`；交付时说明依赖远端 CI。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：所有内置 Agent 入口共享同一预检；缺配置时返回友好提示；`/compact` 与 chat 一致；不引入 raw 401/403 泄漏。
- 执行 `git status --short --branch`、`git diff` 与必要 `git diff --cached`。
- Review：`config_preflight.rs`、`turn_loop.rs` 两处调用点、`client.rs:132`、`format_model_config_guidance()` 文案、`Admin API` 新增 handler。
- 关键点：静态鉴权 Header 覆盖 env_key fallback；`$ENV_NAME` 空引用识别；可选 Header 不被误当鉴权；预检失败仍写入 assistant 历史。
- 复测：单元测试、`test_missing_model_config_returns_guidance_without_model_request`、Admin API 预检 handler。

### 第 2 轮

- 复查第 1 轮修复后的最新 diff，重点确认：
  - Chat 与 `/compact` 都走 preflight。
  - client 入口预检 error 转成合适的业务错误码。
  - Web/Admin 页面调用 `GET /_bifrost/api/agent/config/preflight` 且提示文案与 turn loop 拦截一致。
  - `human_tests/readme.md` 索引与新 case 编号一致。
- 复跑单元、E2E、`human_tests/agent-chat-config-preflight.md` 关键 case。
- 若仍有裸 401 泄漏或某个入口绕过预检，追加第 3 轮。

## 风险与决策

- 已有部署可能依赖 raw 401 触发上游告警。切到预检消息后必须同步告警规则匹配 `AGENT_MODEL_CONFIG_MISSING` 或预检文案，避免误静默。
- `$ENV_NAME` 需要重启 Bifrost 才能读取新值；文案里必须明确“修改后重启”，否则用户会以为不生效并反复修改配置。
- 本地无鉴权 Provider 检测目前只依赖 `env_key`/`api_key`/`env_http_headers` 皆空的隐式规则；若未来出现“本地 Provider 却要求 header”的场景，需要显式增加 `require_auth: bool` 字段避免歧义。
- 已配静态 `Authorization` header 时短路 env_key 检查，可能掩盖“用户误用两套鉴权”问题；日志层建议 info 级记录“static auth header short-circuit”，便于运维排查。
- Admin API 预检 handler 属于 read 类操作，无鉴权变化风险；但 issues 文案里会带出 provider label 与 env 名字，权限模型仍需要沿用 Admin auth（`crates/bifrost-admin/src/auth`）。
