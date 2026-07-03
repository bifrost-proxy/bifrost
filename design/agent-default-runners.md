# Agent Default Runners

## 背景

Bifrost 内置的 AI Agent Chat 支持多种 Runner 后端：内置 `Bifrost Agent` runtime、外部 CLI runner（Codex、Traex 等）、以及未来可能接入的第三方 CLI。运行时需要一个 Runner registry 决定：

- Agent Chat 侧栏的 Runner 选择器展示哪些 Runner。
- IM `/runner` 命令展示哪些 Runner 名称。
- external CLI dispatcher 使用哪个 adapter 拉起进程。
- 用户历史配置和会话中的旧 Runner ID（`bifrost_agent`、`codex`、`traex` / `trae`）如何解析到当前默认 ID。

早期实现只默认写入 `Codex` external runner。用户在缺省数据目录首次打开 Agent 设置时看不到 `Traex`，`Bifrost Agent` 也没有明确的用户可见 ID，导致 IM `/runner` 列表与前端选项不一致。本模块把默认 Runner 集合、旧 ID 兼容、内置 Runner 反序列化、写回 admin 配置的语义写死并可测试化。

## 用户目标验证清单

### 必须实现

- 服务启动 / 首次访问 Agent 配置时自动补齐三个默认 Runner：
  - `Bifrost Agent`：内置 runtime，不写入 external CLI runner map。
  - `Codex`：external CLI runner，adapter 为 `codex`。
  - `Traex`：external CLI runner，adapter 为 `traex`。
- `defaultRunnerId` 默认为 `Codex`。
- Runner ID 使用用户可见名称（大小写与展示一致），底层兼容旧 ID：
  - `bifrost_agent` → `Bifrost Agent`
  - `codex` → `Codex`
  - `traex` / `trae` → `Traex`
- 归一化在读取磁盘配置后执行；如缺失默认 Runner，写回 `admin/im_gateway_external_cli_agent.json`。
- 写回失败只记录 warning，不阻断服务启动。
- 已存在的自定义 Runner 不会被归一化覆盖。
- IM `/runner` 命令展示 `Bifrost Agent`、`Codex`、`Traex`；接受用户输入 `/runner Bifrost Agent` 与旧 `/runner bifrost_agent`。
- `AgentRunnerMode` 反序列化接受 `Bifrost Agent` 作为内置 Runner。

### 必须不破坏

- 用户手动创建、重命名、删除 external CLI runner 的能力保留。
- 已经存在名字为 `Codex` / `Traex` 的用户自定义 Runner 不被 default 归一化重写 adapter / args / env。
- 内置 Runner 不会被误当成 external CLI 执行；`AgentRunnerMode` 分支保持互斥。
- Agent Chat 中已存在会话（历史 `runner_id = "codex"`）打开时仍能正确解析到 `Codex` runner，不出现 "runner not found"。
- 前端 `ExternalCliPanel` 仍显示用户自定义 Runner 列表，允许编辑非默认字段。

### 必须真实验证

- Rust 单元测试覆盖默认 registry、归一化保留自定义、旧 ID 解析、`AgentRunnerMode` 反序列化、`/runner` 列表。
- Rust E2E 通过真实 admin API 验证默认 config 结构。
- human_tests 覆盖真实 CLI 与 UI 操作。

## 产品语义

### 默认 Runner 是系统级基础配置

`Bifrost Agent`、`Codex`、`Traex` 是产品默认可选项；用户可以在此基础上新增自定义 Runner，但不能通过删除让默认 Runner 从配置消失——每次归一化都会补回。

如果用户想让某个默认 Runner 从 UI 隐藏，应使用 `disabled = true` 而不是删除。归一化只补 `missing`，不覆盖 `enabled` 字段的用户选择。

### 用户可见名称即 Runner ID

Runner 在 API 层以 map key 存储：`{"Codex": {...}, "Traex": {...}, ...}`。旧配置 map key 是 `codex` / `traex` 时，读取时兼容解析到新 ID，但归一化后会保留原 key 用于兼容（第一版不 in-place rename，避免破坏用户脚本引用）。请求级 runner override 通过 `effective_config_for_provider_and_runner()` 做兼容解析。

### defaultRunnerId 是首次进入的默认选择

`defaultRunnerId = "Codex"`。用户第一次打开 Agent Chat 而没有个人 preference 时选中 `Codex`；有 preference 时按用户上次选择恢复。

## 技术细节

### 关键 Rust 类型与常量

```rust
pub const DEFAULT_RUNNER_ID_BIFROST_AGENT: &str = "Bifrost Agent";
pub const DEFAULT_RUNNER_ID_CODEX: &str = "Codex";
pub const DEFAULT_RUNNER_ID_TRAEX: &str = "Traex";

pub struct ExternalCliGatewayConfig {
    pub runners: BTreeMap<String, ExternalCliRunner>,
    pub default_runner_id: String,
    ...
}

impl Default for ExternalCliGatewayConfig {
    fn default() -> Self {
        Self {
            runners: BTreeMap::from([
                (DEFAULT_RUNNER_ID_CODEX.into(), ExternalCliRunner::codex_default()),
                (DEFAULT_RUNNER_ID_TRAEX.into(), ExternalCliRunner::traex_default()),
            ]),
            default_runner_id: DEFAULT_RUNNER_ID_CODEX.into(),
            ...
        }
    }
}
```

`ExternalCliConfigStore::new()` 读取 `admin/im_gateway_external_cli_agent.json` 后调用 `normalize_defaults(&mut cfg)`：

1. 补齐 `Codex` / `Traex` 缺失项（不覆盖已存在的 adapter/args/env）。
2. 若 `default_runner_id` 为空或指向不存在 runner，回退到 `Codex`。
3. 若有变更，调用 `persist_config()`；失败只 warn。

`effective_config_for_provider_and_runner(provider, runner)` 归一化 runner override：

```rust
match runner {
    "codex" | "Codex" => Some(cfg.runners.get(DEFAULT_RUNNER_ID_CODEX)),
    "traex" | "trae" | "Traex" => Some(cfg.runners.get(DEFAULT_RUNNER_ID_TRAEX)),
    other => cfg.runners.get(other),
}
```

`AgentRunnerMode` 反序列化：

```rust
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunnerMode {
    #[serde(alias = "bifrost_agent", alias = "Bifrost Agent")]
    BifrostAgent,
    ExternalCli(String),
}
```

### 关键文件路径

- `crates/bifrost-admin/src/im_gateway/external_cli/mod.rs`：`ExternalCliGatewayConfig`、`ExternalCliConfigStore`、归一化与写回。
- `crates/bifrost-admin/src/im_gateway/external_cli/tests.rs`：默认 registry 与归一化单测。
- `crates/bifrost-admin/src/handlers/im_gateway/agent_chat.rs`：请求级 runner override 解析。
- `crates/agent/src/config.rs`：`AgentRunnerMode` 反序列化。
- `web/src/pages/Settings/tabs/imGateway/ExternalCliPanel.tsx`：Runner 列表与编辑 UI。

### 磁盘 schema

`admin/im_gateway_external_cli_agent.json`（示例）：

```json
{
  "default_runner_id": "Codex",
  "runners": {
    "Codex":  { "adapter": "codex",  "enabled": true, "args": [], "env": {} },
    "Traex":  { "adapter": "traex",  "enabled": true, "args": [], "env": {} }
  }
}
```

`Bifrost Agent` 是内置 runtime，不写入 runners map。

## CLI 交互

- IM `/runner` 列表展示：`Bifrost Agent`、`Codex`、`Traex`（按 registry 加自定义 Runner 追加）。
- IM `/runner <name>`：接受 `/runner Bifrost Agent`、`/runner Codex`、`/runner Traex`；同时接受旧 `/runner bifrost_agent`、`/runner codex`、`/runner traex`、`/runner trae`。
- `bifrost agent` CLI（如果已实现）也走同一 registry。

## Web UI 交互

- Settings > IM Gateway > External CLI：
  - 首次打开列表包含 `Codex` 与 `Traex`（enabled）。
  - `Bifrost Agent` 作为内置 Runner 在 Runner 选择器另开一行，不出现在 external CLI table 中。
  - `defaultRunnerId` 显示为 `Codex`，可切换到任意 enabled Runner。
  - 用户可新增自定义 Runner；归一化不会覆盖 adapter/args/env。
- Agent Chat：Runner 选择器同样展示三个默认 Runner；选中 `Bifrost Agent` 时走内置 runtime，选中 external Runner 时走 external CLI dispatcher。

## Admin API

- `GET /api/im-gateway/chat/config`：返回归一化后的 `ExternalCliGatewayConfig`。默认响应包含 `defaultRunnerId = "Codex"`、`runners.Codex.adapter = "codex"`、`runners.Traex.adapter = "traex"`。
- `PUT /api/im-gateway/chat/config`：用户更新配置。写入后再走归一化以保证默认 Runner 不消失。
- 请求级 override（例如 `/api/im-gateway/agent/chat` 传 `runner`）通过 `effective_config_for_provider_and_runner()` 解析。

## Sync / 导入导出 / 分享边界

- external CLI 配置属于本机私有配置，不参与 rule sync。
- 导入 external CLI 配置时仍走归一化，保证默认 Runner 齐全。
- 不支持通过 share URL 覆盖 external CLI runner，避免第三方链接注入 adapter/args。

## 实现切分

### Phase 1：默认 registry 与归一化

- 引入 `DEFAULT_RUNNER_ID_*` 常量。
- `ExternalCliGatewayConfig::default()` 返回 Codex + Traex。
- `ExternalCliConfigStore::new()` 调用归一化并按需 persist。
- 写回失败只 warn。

### Phase 2：旧 ID 兼容

- `effective_config_for_provider_and_runner()` 兼容 `codex` / `traex` / `trae`。
- `AgentRunnerMode` 反序列化接受 `Bifrost Agent` 与 `bifrost_agent`。
- IM `/runner` slash 解析同一套 alias 表。

### Phase 3：前端与 IM 文案

- `ExternalCliPanel` 与 Runner 选择器展示新 ID。
- IM `/runner` 列表使用新 ID。

### Phase 4：文档与 human_tests

- 更新 `human_tests/agent-builtin-tools-completeness.md` 覆盖默认 Runner 与旧 ID 兼容。
- 更新 README/docs 中 Runner 相关文案（如有）。

## 测试方案

### 单元测试

- `external_cli::tests::default_registry_contains_codex_and_traex_enabled`：验证默认 registry 含 Codex/Traex 且 enabled。
- `external_cli::tests::normalize_defaults_adds_missing_runners_without_overwriting_custom`：验证归一化只补 missing，不覆盖自定义 adapter/args/env。
- `external_cli::tests::normalize_defaults_persists_when_changed`：验证归一化产生变更时写回磁盘。
- `external_cli::tests::normalize_defaults_persist_failure_warns_but_returns_ok`：验证写回失败只 warn，不阻断。
- `external_cli::tests::legacy_runner_id_resolves_to_current_default`：验证 `codex`/`traex`/`trae` 请求级 override 能解析到当前默认 ID。
- `agent::config::tests::agent_runner_mode_accepts_bifrost_agent_alias`：验证 `Bifrost Agent` 与 `bifrost_agent` 都能反序列化为内置 Runner。
- `im_gateway::agent_slash::tests::runner_command_lists_default_runners`：验证 `/runner` 列表包含三项且接受新旧 alias。

### E2E 测试

- `bifrost-e2e` 中 `im_gateway_agent_config_get`：启动真实 Admin API，`GET /api/im-gateway/chat/config` 断言：
  - `defaultRunnerId == "Codex"`
  - `runners.Codex.adapter == "codex"`
  - `runners.Traex.adapter == "traex"`
- 若需要覆盖 external CLI 拉起链路，可扩展 `test_im_agent_external_cli_runner.sh`（如存在），断言 `runner: "codex"` 与 `runner: "Codex"` 都能拉起 Codex adapter。

### 真实场景测试 human_tests

- 更新 `human_tests/agent-builtin-tools-completeness.md`：
  - TC-DR-01：首次启动 Agent 设置页展示 `Bifrost Agent`、`Codex`、`Traex`。
  - TC-DR-02：删除磁盘 `admin/im_gateway_external_cli_agent.json` 中 `Traex` 后重启，Traex 被自动补回。
  - TC-DR-03：IM `/runner` 列表包含三项，`/runner bifrost_agent` 与 `/runner Bifrost Agent` 都能切换到内置 Runner。
  - TC-DR-04：历史 session `runner_id = "codex"` 打开正常，不报 "runner not found"。
  - TC-DR-05：新增自定义 Runner `MyCli` 后归一化不覆盖。

所有 human_tests 使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 与 `--no-system-proxy`。

### 覆盖率与项目校验

- `cargo test -p bifrost-admin external_cli::tests::`
- `cargo test -p bifrost-agent agent_runner_mode_accepts_bifrost_agent_alias`
- `cargo test -p bifrost-e2e im_gateway_agent_config_get`
- 收尾按项目规则执行 `rust-project-validate`，并至少执行一次 `cargo test --workspace --all-features`。
- 本机 no-local-coverage 约定生效时不执行 `make coverage`。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：默认 registry、归一化只补 missing、旧 ID 兼容、内置 Runner 不进 external CLI 执行、IM `/runner` 列表。
- 复核 diff：`external_cli/mod.rs`、`external_cli/tests.rs`、`agent_chat.rs`、`agent/config.rs`、`ExternalCliPanel.tsx`、`human_tests`。
- 重点 review：自定义 Runner 是否被误覆盖；`AgentRunnerMode` 是否漏掉 alias；`/runner` 是否把 `Bifrost Agent` 当 external CLI 执行；旧 session 打开是否报错。
- 复测：单测 + E2E `im_gateway_agent_config_get` + human_tests。

### 第 2 轮

- 复核第 1 轮修复后的 diff，重点看写回失败路径 warning 稳定性、grid/select UI 的默认高亮。
- 复跑受影响测试；如新增默认 Runner，同步更新 human_tests 与前端文案。

## 风险与决策点

- 是否 in-place rename 旧 map key `codex` → `Codex`：第一版不 rename，避免破坏用户脚本引用；只做读取兼容。
- 是否允许通过 API 删除默认 Runner：不允许（`normalize_defaults` 会补回）；如果产品希望允许隐藏，用 `enabled = false`。
- 内置 `Bifrost Agent` 是否需要单独 `disabled` 开关：第一版不需要，Bifrost Agent 一直可用。
- 若后续新增默认 Runner，必须同步更新 `human_tests/agent-builtin-tools-completeness.md`、`ExternalCliPanel` 文案、以及 IM `/runner` 展示。
