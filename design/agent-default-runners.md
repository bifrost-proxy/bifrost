# Agent Default Runners

## 功能模块

AI Agent Runner registry 在服务启动时必须保证默认 Runner 可用。默认用户可见 Runner 集合为：

- `Bifrost Agent`：内置 Agent runtime，不写入 external CLI runner map。
- `Codex`：外部 CLI Runner，adapter 为 `codex`。
- `TreeX`：外部 CLI Runner，adapter 为 `traex`。

Runner ID 使用用户可见名称。为兼容历史配置和会话，后端继续接受 `bifrost_agent`、`codex`、`traex` / `trae` 等旧 ID，并在执行 external runner 时解析到新的默认 ID。

## 实现逻辑

- `ExternalCliGatewayConfig::default()` 默认创建 enabled 的 `Codex` 和 `TreeX` external runners，`defaultRunnerId` 为 `Codex`。
- `ExternalCliConfigStore::new()` 读取磁盘配置后执行归一化；如果缺少默认 Runner，会写回 `admin/im_gateway_external_cli_agent.json`。写回失败只记录 warning，不阻断服务启动。
- `effective_config_for_provider_and_runner()` 对请求级 runner override 做兼容解析：旧 `codex` 指向 `Codex`，旧 `traex` / `trae` 指向 `TreeX`。
- IM `/runner` 列表展示 `Bifrost Agent`，同时接受 `/runner Bifrost Agent` 和旧 `/runner bifrost_agent`。
- `AgentRunnerMode` 反序列化接受 `Bifrost Agent` 作为内置 Runner。

## 依赖项

- `crates/bifrost-admin/src/im_gateway/external_cli/mod.rs`
- `crates/bifrost-admin/src/handlers/im_gateway/agent_chat.rs`
- `crates/agent/src/config.rs`
- `web/src/pages/Settings/tabs/imGateway/ExternalCliPanel.tsx`

## 测试方案

- 单元测试：
  - 默认 registry 包含 enabled 的 `Codex` / `TreeX`。
  - 旧配置归一化补齐 `Codex` / `TreeX` 且不覆盖自定义 Runner。
  - 旧 ID `codex` / `traex` 解析到新默认 ID。
  - `Bifrost Agent` 可反序列化为内置 Runner。
  - IM `/runner` 列表展示 `Bifrost Agent`、`Codex`、`TreeX`。
- E2E：
  - `im_gateway_agent_config_get` 启动真实 Admin API，检查 `/api/im-gateway/chat/config` 默认返回 `defaultRunnerId=Codex`，且 `runners.Codex.adapter=codex`、`runners.TreeX.adapter=traex`。
- human_tests：
  - 更新 `human_tests/agent-builtin-tools-completeness.md`，按真实命令执行默认 Runner 配置与旧 ID 兼容检查。

## Review/Fix/Test 闭环方案

- 第 1 轮：复核默认创建、旧 ID 兼容、内置 Runner 不被当成 external CLI 执行；运行 targeted Rust 单测。
- 第 2 轮：复查 diff、设计文档、human_tests 和真实 API 行为；复跑 targeted E2E/human_tests。

## 校验要求

- 先执行 E2E/human_tests，再执行 rust-project-validate。
- 收尾执行 `make coverage`；如果 E2E 覆盖环境不可用，退化为 `make coverage-unit` 并说明原因。

## 文档更新要求

- 本设计文档随默认 Runner 行为维护。
- 若后续新增默认 Runner，必须同步更新 `human_tests/agent-builtin-tools-completeness.md` 与前端 Runner 文案。
