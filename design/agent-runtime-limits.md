# Agent Runtime Limits 与超时策略

## 背景

Bifrost 内置 Agent runtime 在真实 IM Gateway 会话、Web Agent Chat 和 Admin API 端到端链路里，经常需要在一次用户请求中完成多轮工具调用（read/edit/patch/list/exec/mcp tool）。老版本 runtime 的默认迭代上限和分层超时都源于开发期的短链路，一旦触碰到真实长链路会话就会提前失败：

- `max_turn_iterations = 20`：真实 coding agent 场景常常 25 轮起步，30 次上下就被硬中断，报 `exceeded maximum iterations (30)`。
- `request_timeout_secs = 120`：慢 provider 或 reasoning 类模型很容易超过 120 秒返回首 token，导致误报失败。
- MCP `startup_timeout_sec = 30`、`tool_timeout_sec = 60`：MCP server 初始化偏慢或工具本身耗时略长时，会被过短默认值直接判死。
- `AgentClient` 内部 `reqwest::Client::builder().timeout(Duration::from_secs(300))`：与 per-request timeout 并存，形成隐藏的“第二层 5 分钟上限”，用户即便把 `request_timeout_secs` 调到 600 也仍然被 300 秒兜底截断，问题极难排查。

本设计把默认 runtime limits 提升到接近真实代理执行的量级，并去掉 builder 层隐藏上限，保证 runtime 行为完全由显式配置控制。设计不改变任何字段名，不破坏 TOML/JSON 反序列化，不影响 external CLI runner；只调整默认值和 `AgentClient` builder 语义。

## 用户目标验证清单

### 必须实现

- `AgentConfig::DEFAULT_MAX_TURN_ITERATIONS = 1000`，`get_max_turn_iterations()` 与 `default()` 返回一致。
- `AgentConfig::DEFAULT_REQUEST_TIMEOUT = 600` 秒，`get_request_timeout_secs()` 与 `default()` 返回一致。
- `AgentConfig::DEFAULT_BACKGROUND_TERMINAL_TIMEOUT_MS = 300_000`（5 分钟），与 Codex unified exec 后台轮询上限对齐。
- MCP `DEFAULT_STARTUP_TIMEOUT_SEC = 600`、`DEFAULT_TOOL_TIMEOUT_SEC = 600`；显式配置仍然优先。
- `AgentClient::new()` 不再在 `reqwest::Client::builder()` 上写死 300 秒 timeout；per-request `.timeout(Duration::from_secs(effective.request_timeout_secs))` 保留。
- 真实 mock model 连续触发 35 次以上工具调用后仍能正常收尾，不出现 `exceeded maximum iterations (30)` 或 300 秒 builder 截断。
- Admin API `GET /_bifrost/api/im-gateway/agent` 返回的默认配置反映新的常量值。
- `im-gateway/agent` PATCH 后再 GET 能正确读回自定义值，不被新默认值覆盖。

### 必须不破坏

- 用户在 TOML/JSON 里显式设置的 `max_turn_iterations`、`request_timeout_secs`、`background_terminal_max_timeout`、`startup_timeout_sec`、`tool_timeout_sec` 仍然优先于默认值。
- `AgentConfig::merge()` 的覆盖语义（overlay 的 `Some(x)` 覆盖 base）不变；老的 TOML 反序列化路径不受影响。
- MCP `start_one_server()` 仍走 `unwrap_or(DEFAULT_*)` 回退，不新增其他隐藏兜底。
- Agent Chat SSE、IM Gateway progress card、External Runner 流程和 status/统计口径保持不变；只是不会再因为默认值过低误报失败。
- Session persistence、compaction、token snapshot、goal accounting 逻辑无关联改动。
- CLI `bifrost agent chat/status/config` 输出字段名、顺序、类型不变。

### 必须真实验证

- 单元测试断言 `AgentConfig::default()` 三个字段的新数值，且 accessor 与常量一致。
- 单元测试断言 MCP 默认常量 = 600，`unwrap_or` 分支被真实触发。
- 单元测试断言 `AgentClient::new()` 生成的 `reqwest::Client` 没有 client-level timeout（例如通过反射或行为验证：使用一个 700 秒的 mock server 仍能收到响应）。
- E2E 脚本 `test_agent_loop_runtime_limits.sh` 真实启动 bifrost + mock model server，触发 35 轮以上工具调用完成整个 turn。
- Admin API GET/PATCH round-trip 真实验证。
- human_tests 逐条执行并记录 CLI/HTTP 实际输出。

## 现状与问题

### 相关代码

- `crates/agent/src/config.rs`
  - 常量：`DEFAULT_MAX_TURN_ITERATIONS`、`DEFAULT_REQUEST_TIMEOUT`、`DEFAULT_BACKGROUND_TERMINAL_TIMEOUT_MS`
  - `AgentConfig::default()`、`get_request_timeout_secs()`、`get_max_turn_iterations()`、`get_background_terminal_max_timeout()`
  - `merge()`：field-by-field overlay
- `crates/agent/src/mcp/mod.rs`
  - `DEFAULT_STARTUP_TIMEOUT_SEC`、`DEFAULT_TOOL_TIMEOUT_SEC`
  - `start_one_server()`：使用 `unwrap_or(DEFAULT_*)`
- `crates/agent/src/client.rs`
  - `AgentClient::new()`：`reqwest::Client::builder().timeout(Duration::from_secs(300)).build()`
  - `send_request()`：per-request `.timeout(Duration::from_secs(effective.request_timeout_secs))`
- `crates/bifrost-admin/src/handlers/im_gateway.rs`
  - GET/PATCH agent config 入口

### 用户可观察的旧问题

- IM Gateway 长任务在第 30 轮工具调用时被 `exceeded maximum iterations (30)` 截断。
- Chat Completions 请求返回慢时，即使 `request_timeout_secs=600` 也在 5 分钟被 client builder timeout 触发 hyper 层 `connection reset` / `request canceled`。
- MCP server 初始化偏慢时经常被 30 秒 startup timeout 判死。

## 产品语义

- Bifrost Agent 默认应当能支撑真实 coding agent 的一次完整任务：几百到上千轮工具调用、单模型请求最长 10 分钟量级、MCP server 冷启和长工具单次 10 分钟量级。
- 用户仍然可以按需 clamp：例如把 `request_timeout_secs` 调回 120 秒，以做严格的 SLA 控制。
- Client builder 层不再持有独立 timeout，语义变为“per-request timeout 就是唯一权威”。这样对未来引入 provider-specific timeout override（如 reasoning 模型）没有二次覆盖风险。

## 技术细节

### 1. 调整 AgentConfig 默认值

修改 `crates/agent/src/config.rs`：

```rust
pub const DEFAULT_MAX_TURN_ITERATIONS: u32 = 1000;
pub const DEFAULT_REQUEST_TIMEOUT: u64 = 600;
pub const DEFAULT_BACKGROUND_TERMINAL_TIMEOUT_MS: u64 = 300_000;
```

同步：

- 字段文档注释更新为新的默认值与量级说明。
- `AgentConfig::default()` 内 `Some(DEFAULT_*)` 直接引用常量。
- `get_*_secs()` / `get_*_ms()` 的 `unwrap_or(Self::DEFAULT_*)` 保持一致。
- `test_default_config` 及 `test_merge_config_overlay` 相关断言更新。
- 老的 TOML fixtures（例如 `max_turn_iterations = 30`）仍然合法：merge overlay 仍胜出，测试断言保持覆盖。

### 2. 统一 MCP 默认超时

修改 `crates/agent/src/mcp/mod.rs`：

```rust
const DEFAULT_STARTUP_TIMEOUT_SEC: u64 = 600;
const DEFAULT_TOOL_TIMEOUT_SEC: u64 = 600;
```

`start_one_server()`：

- `Duration::from_secs(config.tool_timeout_sec.unwrap_or(DEFAULT_TOOL_TIMEOUT_SEC))`
- `Duration::from_secs(config.startup_timeout_sec.unwrap_or(DEFAULT_STARTUP_TIMEOUT_SEC))`

保留：显式配置覆盖默认；`stop_one_server()` 和错误分类语义不变。

### 3. 去掉 AgentClient 隐式 300 秒上限

修改 `crates/agent/src/client.rs`：

- `AgentClient::new()` 从：
  ```rust
  reqwest::Client::builder()
      .timeout(Duration::from_secs(300))
      .build()
  ```
  改为：
  ```rust
  reqwest::Client::builder()
      // no client-level timeout: per-request .timeout() is authoritative
      .build()
  ```
- `send_request()` 中的 `.timeout(Duration::from_secs(effective.request_timeout_secs))` 保留。
- 保留其他 builder 配置（如 gzip、user agent、TLS 信任），仅移除 `.timeout(...)`。

### 4. Admin API 默认值透出

- `GET /_bifrost/api/im-gateway/agent` 返回结构中 `agent.max_turn_iterations`、`agent.request_timeout_secs`、`agent.background_terminal_max_timeout` 使用 `AgentConfig::default()` 得到的值（对未持久化字段回退到默认）。
- `PATCH` 保持既有 field-by-field 半覆盖语义，不需要因默认值改动而更改序列化 shape。

### 5. 兼容性与迁移

- 老配置文件中显式写入的 `max_turn_iterations = 30` 之类值仍然生效，不做隐式改写。
- 若发现用户有依赖 300 秒 client timeout 的历史脚本，需要在 CHANGELOG 中提示：per-request `request_timeout_secs` 才是权威值。

## CLI / Admin API / Web

### CLI

- `bifrost agent status` / `bifrost agent config show` 输出字段不变；实际数值取决于配置合并结果。
- `bifrost agent config get max-turn-iterations` 返回 `1000`（未显式配置时）。

### Admin API

- `GET /_bifrost/api/im-gateway/agent`
  ```json
  {
    "agent": {
      "max_turn_iterations": 1000,
      "request_timeout_secs": 600,
      "background_terminal_max_timeout": 300000
    }
  }
  ```
- `PATCH /_bifrost/api/im-gateway/agent` 显式设值后再 GET 回读一致。
- MCP 相关 endpoint 不变；`startup_timeout_sec`/`tool_timeout_sec` 若未设置，实际生效值为 600。

### Web UI

- Settings → Agent 页面若展示 default hint，需要同步文案。第一版可仅调整常量数值，Web UI 展示会自动跟随。

## Sync 边界

- Agent runtime 默认值属于本地行为参数，不作为跨设备 sync 内容传播。
- 若用户显式写入 `bifrost-config.toml`，同步策略沿用现有 config sync 边界（本方案不改）。

## 实现切分

### Phase 1：常量与访问器

- 修改 `crates/agent/src/config.rs` 三个常量与 `default()`。
- 修改 `crates/agent/src/mcp/mod.rs` 两个常量。
- 修改 `crates/agent/src/client.rs` 去掉 builder 300 秒。
- 更新对应单元测试断言。

### Phase 2：Admin API 与 CLI

- 验证 `GET /agent` 返回新默认值。
- 验证 CLI `agent config show` 输出一致。
- 若 Web Settings 有硬编码默认，同步更新。

### Phase 3：E2E 与 human_tests

- 新增 `e2e-tests/tests/test_agent_loop_runtime_limits.sh`。
- 新增 `human_tests/agent-loop-timeouts.md` 并在 `human_tests/readme.md` 中登记。
- 复用现有 mock model server 或新增 `mock-agent-server` 支持连续 35+ 次 tool call。

### Phase 4：文档与 CHANGELOG

- 更新 `docs/agent.md` / `docs-en/agent.md` 中的默认值表。
- CHANGELOG 中提示 client builder timeout 变更，避免用户依赖旧兜底。

## 测试方案

### 单元测试

1. `crates/agent/src/config.rs::test_default_config`
   - 断言 `AgentConfig::default().get_request_timeout_secs() == 600`
   - 断言 `AgentConfig::default().get_max_turn_iterations() == 1000`
   - 断言 `AgentConfig::default().get_background_terminal_max_timeout() == 300_000`
2. `crates/agent/src/config.rs::test_merge_config_overlay`
   - 已有 `max_turn_iterations: Some(60)` overlay 后仍为 60，验证默认值提升未干扰 merge。
3. `crates/agent/src/config.rs::test_toml_deserialize`
   - 老 fixture `max_turn_iterations = 30` 反序列化后仍为 30。
4. `crates/agent/src/mcp/mod.rs::test_startup_timeout_defaults`
   - `MCPConfig { startup_timeout_sec: None, tool_timeout_sec: None, .. }` 走 `unwrap_or(600)`。
5. `crates/agent/src/client.rs::test_client_has_no_builder_timeout`
   - `AgentClient::new()` 生成的 client 对 800 秒响应仍能等待成功（用 tokio 时间控制或 mock server）。
   - 或使用 `#[cfg(test)]` 暴露 `builder_timeout()` 返回 `None`。

### E2E 测试

新增 `e2e-tests/tests/test_agent_loop_runtime_limits.sh`：

1. `GET /_bifrost/api/im-gateway/agent`：断言 `max_turn_iterations=1000` / `request_timeout_secs=600` / `background_terminal_max_timeout=300000`。
2. PATCH mock provider 后调 `POST /_bifrost/api/im-gateway/agent/chat`：
   - mock 触发 35 次 `list_directory` 工具调用。
   - 最终 assistant final response 返回。
   - session detail 中 `tool_calls.len() > 30`。
   - stderr 无 `exceeded maximum iterations`。
3. mock 模拟 400 秒才返回首 token 的 chat completion，验证 request 成功而不是被 300 秒截断。
4. PATCH `max_turn_iterations = 5`，再触发 6 次工具调用，验证仍能显式收敛到旧行为（不被新默认覆盖）。

### 真实场景测试（human_tests）

新增 `human_tests/agent-loop-timeouts.md`：

- `TC-AL-01`：读取默认 Agent 配置，确认三项默认值。
- `TC-AL-02`：执行真实黑盒 E2E 脚本，验证 35 轮工具调用成功。
- `TC-AL-03`：PATCH 600 秒请求 / 300 秒后台终端 timeout，再 GET 回读一致。
- `TC-AL-04`：模拟 400 秒慢响应 provider，确认 client 不再被 300 秒截断。

所有 human_tests 启动 Bifrost 必须使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-agent config default_config merge mcp startup client builder_timeout`
- `cargo test --workspace --all-features`
- `rust-project-validate`
- 本机 no-local-coverage 生效时不跑 `make coverage`，交付时说明并依赖远端 CI。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核三处常量与 accessor 是否一致，`AgentConfig::default()` 是否落到常量。
- 复核 `AgentClient::new()` builder 中是否还残留 `.timeout(...)`。
- 复核 MCP `start_one_server()` 是否仍是 `unwrap_or(DEFAULT_*)`。
- 运行单元测试、mcp 测试、client 测试；运行 E2E 脚本。
- 抓取 `git status --short` 与 `git diff`，确认无残留 println/dbg。

### 第 2 轮

- 复查 Admin API 返回结构和 Web Settings 展示是否与新默认值一致。
- 若第 1 轮修复了兼容性问题，再跑一次 E2E 与 human_tests。
- 复查文档与 CHANGELOG 是否同步。

## 风险与决策点

- **默认值提升与资源占用**：`max_turn_iterations=1000` 与 600 秒 request timeout 可能让异常工具链条更晚被发现。缓解措施：runtime metrics + IM 侧 goal accounting 仍在跟踪 turn 数量与 token 用量，Web UI Status 面板可视化。
- **移除 client builder timeout 的 blast radius**：per-request timeout 是唯一权威值；如果未来某处忘记设 `.timeout(...)`，请求可能无限挂起。缓解：`AgentClient::send_request` 是唯一出口且必须显式带 timeout，通过单测锁定。
- **老配置兼容**：显式写入短 timeout 的用户不受影响；仅默认值提升。若用户依赖“client 层 300 秒兜底”做隐性熔断，需要在升级说明中提示改为显式 `request_timeout_secs`。
- **MCP 长 startup**：600 秒对于本地 stdio MCP server 通常足够；如果外部远程 MCP server 冷启更慢，可通过显式 `startup_timeout_sec` 拉高。
- **未来分层**：若需要按 provider（reasoning 模型 vs. chat 模型）分别设定 timeout，可以在 provider-config 中扩展，仍不需要 client builder 兜底。
