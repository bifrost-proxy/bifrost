# Remote Traffic CLI 枚举体瘦身设计

## 背景

`crates/bifrost-cli/src/cli.rs` 中的 `RemoteTrafficCommands` 直接把 `remote traffic list` 的全部过滤参数内联在 `List` 变体上。

由于该变体包含大量 `Option<String>` 字段，`cargo clippy --workspace --all-targets --all-features -- -D warnings` 在 CI 中触发 `clippy::large-enum-variant`，阻塞 `bifrost-cli` 及其测试目标构建。

## 目标

- 消除 `RemoteTrafficCommands` 的 large enum variant lint
- 保持 `bifrost remote traffic list/get/search` 的 CLI 参数名、帮助文案和行为不变
- 不改变 caller 侧构造 `traffic.list` / `traffic.get` / `traffic.search` 的 `args_json` 语义

## 实现方案

### 1. 拆分 `remote traffic list` 参数结构

在 `crates/bifrost-cli/src/cli.rs` 中新增 `RemoteTrafficListArgs`：

- 使用 `#[derive(Args, Clone, Debug)]`
- 保留 `remote traffic list` 当前的全部参数定义
- 由 `RemoteTrafficCommands::List` 持有 `Box<RemoteTrafficListArgs>`

这样可将大字段从枚举体本身移到堆上，缩小枚举体尺寸，满足 clippy 要求。

### 2. 保持命令构造逻辑不变

在 `crates/bifrost-cli/src/commands/remote.rs` 中：

- `RemoteTrafficCommands::List(list_args)` 解构为独立结构
- 继续按原有字段生成 `traffic.list` 的 `args_json`

预期结果：

- relay/openCall 收到的 `limit/cursor/direction/.../is_tunnel` 参数不变
- 现有远程调用协议无需调整

## 测试方案

### 单元测试

- 更新 `test_build_remote_command_for_traffic_list_includes_all_filters`
- 断言 `traffic.list` 的 `args_json` 仍完整包含所有过滤参数

### E2E 测试

- 复用已有 `e2e-tests/tests/test_remote_invoke_e2e.sh`
- 重点验证 `remote traffic list` / `remote traffic search` 参数透传未回归

### 真实场景测试（human_tests）

更新 `human_tests/remote-invoke.md`：

- 新增回归用例，验证本次“CLI 结构瘦身”后 `remote traffic list` / `remote traffic search` / `remote traffic get` 依然可用
- 记录实际执行结果

同步更新 `human_tests/readme.md` 索引表与用例数量。

## 校验要求

- 先执行相关 E2E/定向测试
- 再执行 `cargo fmt --all -- --check`
- 再执行 `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- 执行 `cargo test -p bifrost-cli`
- 至少执行一次 `cargo test --workspace --all-features`
- 按修改范围执行 `bash scripts/ci/local-ci.sh --skip-e2e`

## 文档更新要求

- 本次改动不引入新的 CLI 参数和用户可见能力，`README.md` 无需更新
- `design/` 与 `human_tests/` 必须同步更新，满足仓库开发门禁
