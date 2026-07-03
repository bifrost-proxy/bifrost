# Remote Traffic CLI 枚举体瘦身设计

> 状态：已实现 | 更新时间：2026-07-03

## 背景

`crates/bifrost-cli/src/cli/remote.rs` 的 `RemoteTrafficCommands` 承载 `bifrost remote traffic list/get/search` 三条子命令。旧实现把 `remote traffic list` 的全部过滤参数直接内联在 `List` 变体上：

- `Option<String>` / `Option<u64>` / `Vec<String>` 类字段随着功能扩展越来越多（时间窗、JSONPath、header eq、value size、body regex、is_tunnel、direction、cursor、include masks 等）。
- 该枚举通过 `crates/bifrost-cli/src/cli.rs` 的 `pub mod remote; pub use remote::*;` 对外暴露，几乎所有 CLI 子命令派发都会引用它。
- 由于 `List` 变体过大，`cargo clippy --workspace --all-targets --all-features -- -D warnings` 会触发 `clippy::large-enum-variant`，让所有 enum 实例都以 `List` 尺寸对齐，浪费栈空间并阻塞 CI。

Clippy 报错样例：

```
error: large size difference between variants
   --> crates/bifrost-cli/src/cli/remote.rs:1225
    |
1227 |     List(RemoteTrafficListArgs) // 400 bytes
1229 |     Get(...)                    //  56 bytes
1230 |     Search(...)                 //  32 bytes
    |
    = help: put the large variant into a Box or consider refactoring
```

## 用户目标验证清单

### 必须实现

- 消除 `RemoteTrafficCommands` 的 `clippy::large-enum-variant` 阻塞。
- `bifrost remote traffic list/get/search` 的 CLI 参数名、帮助文案与行为保持完全不变。
- caller 侧构造的 `traffic.list` / `traffic.get` / `traffic.search` `args_json` 语义不变；remote query 主链路继续使用 `CanonicalQueryCommand`。
- 单元测试断言所有过滤参数依然被正确写入 `args_json`。

### 必须不破坏

- clap derive 派发路径与现有帮助输出。
- `RemoteTrafficCommands` 的公开 `pub use remote::*;` 出口不改路径。
- `commands/remote.rs` 中 `build_remote_command_for_traffic_list` 生成 `args_json` 的字段顺序、值格式与旧版一致，避免 relay / worker 侧 hash / 摘要差异。

### 必须真实验证

- 真实 CLI `bifrost remote traffic list --limit 5 --host example.com --from 2026-06-01T00:00:00Z --to 2026-06-30T23:59:59Z --status-min 200 --status-max 299 --body-regex foo` 参数透传无回归。
- `bifrost remote traffic search` / `bifrost remote traffic get` 参数与响应体和瘦身前一致。

## 产品语义

- CLI 变体保持不变：`RemoteTrafficCommands::List`、`Get`、`Search`。
- `List` 变体从直接持有大结构改为持有 `Box<RemoteTrafficListArgs>`：栈上仅 8 字节指针，堆上完整参数结构。
- 命令处理层解构 `Box` 后按原字段逐一构造 `args_json`；对 caller / relay / worker 完全透明。

## 技术细节

### 1. 拆分 `remote traffic list` 参数结构

`crates/bifrost-cli/src/cli/remote.rs`：

```rust
#[derive(Args, Clone, Debug)]      // line 1025
pub struct RemoteTrafficListArgs {
    #[arg(long)] pub limit: Option<u64>,
    #[arg(long)] pub cursor: Option<String>,
    #[arg(long)] pub direction: Option<String>,
    #[arg(long)] pub host: Option<String>,
    #[arg(long)] pub method: Option<String>,
    #[arg(long)] pub status_min: Option<u16>,
    #[arg(long)] pub status_max: Option<u16>,
    #[arg(long)] pub from: Option<String>,
    #[arg(long)] pub to: Option<String>,
    #[arg(long)] pub body_regex: Option<String>,
    #[arg(long)] pub header_eq: Vec<String>,
    #[arg(long)] pub value_size_min: Option<u64>,
    #[arg(long)] pub value_size_max: Option<u64>,
    #[arg(long)] pub jsonpath: Option<String>,
    #[arg(long)] pub include_body: bool,
    #[arg(long)] pub include_headers: bool,
    #[arg(long)] pub is_tunnel: Option<bool>,
    // ...其余过滤参数保持一致
}

#[derive(Subcommand, Clone, Debug)]     // line 1225
pub enum RemoteTrafficCommands {
    List(Box<RemoteTrafficListArgs>),   // line 1227
    Get { /* 保持原字段 */ },
    Search(Box<RemoteSearchArgs>),      // 已同样 Box 化
}
```

### 2. 构造逻辑保持不变

`crates/bifrost-cli/src/commands/remote.rs`（line 7395 及以下）：

```rust
match action {
    RemoteTrafficCommands::List(list_args) => {
        let list_args = list_args.as_ref();
        build_remote_command_for_traffic_list(list_args, ...)
    }
    RemoteTrafficCommands::Get { id, request_body, response_body } => { ... }
    RemoteTrafficCommands::Search(search_args) => { ... }
}
```

`build_remote_command_for_traffic_list` 继续按原有字段生成 `traffic.list` 的 `args_json`，`open_call` 请求体、`command_summary` 摘要、`masked_args_json` 均无差异。

### 3. Search 与 Get 变体

- `Search(Box<RemoteSearchArgs>)`：`RemoteSearchArgs` 同样属于大结构，一并 Box 化避免第二次 `large-enum-variant`。
- `Get`：字段少（`id`、`--request-body`、`--response-body`），保留内联结构。

### CLI + Web + Admin API

- CLI 表面 API 不变：命令名、参数名、帮助、退出码保持一致。
- Web / Admin API 不涉及本次改动。
- Remote Invoke opcode `traffic.list` / `traffic.get` / `traffic.search` 的 `args_json` schema 保持稳定。

### Sync 边界

- 仅是 CLI 结构瘦身，不涉及 sync、持久化、跨端协议。
- Recent Calls / relay 存储字段无变化。

## Phase 1-4 拆分

### Phase 1：结构拆分

- 新增 `RemoteTrafficListArgs` 结构，`List` 变体改为 `Box<RemoteTrafficListArgs>`。
- 同步 `RemoteSearchArgs` 也 Box 化。
- clap derive 编译通过；`cargo build -p bifrost-cli` 无警告。

### Phase 2：命令派发解构

- `commands/remote.rs` 中 `match action` 分支解构 `Box`。
- `build_remote_command_for_*` 保持原签名。
- 检查 `args_json` 序列化字段顺序与旧版一致。

### Phase 3：clippy 与测试

- `cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过。
- `cargo test -p bifrost-cli` 通过，重点关注 `test_build_remote_command_for_traffic_list_includes_all_filters`（line 7764）。
- `cargo test --workspace --all-features` 通过。

### Phase 4：真实场景回归

- `bash e2e-tests/tests/test_remote_invoke_e2e.sh` 覆盖 `remote traffic list/search/get` 参数透传。
- 更新 `human_tests/remote-invoke.md` 与 `human_tests/readme.md`。

## 测试方案

### 单元测试

- `cargo test -p bifrost-cli remote::tests::test_build_remote_command_for_traffic_list_includes_all_filters`：断言 `traffic.list` 的 `args_json` 依然含 `limit / cursor / direction / host / method / status_min / status_max / from / to / body_regex / header_eq / value_size_min / value_size_max / jsonpath / include_body / include_headers / is_tunnel` 全部字段。
- `cargo test -p bifrost-cli remote::tests::test_build_remote_command_for_traffic_search_uses_streaming_command`：搜索模式仍走 canonical query，`args_json` 保留过滤字段。
- `cargo test -p bifrost-cli remote::tests::test_build_remote_command_for_traffic_get_includes_body_flags`：`traffic.get` 携带 `id / request_body / response_body`。
- `cargo test -p bifrost-cli remote::tests::test_build_open_call_command_summary_uses_label_and_args_json`：`command_summary` 稳定摘要。

### E2E 测试

- `bash e2e-tests/tests/test_remote_invoke_e2e.sh`：`TC-RI-02/03/04` 涵盖 `traffic list/search/get` 参数透传与响应结构。
- `bash e2e-tests/tests/test_remote_search_traffic_cli_isomorphic_e2e.sh`：覆盖 CLI 直连与 remote invoke 两条链路对同一 traffic dataset 的语义等价。

### 真实场景测试（human_tests）

`human_tests/remote-invoke.md`：新增回归用例：

- `TC-RI-回归-CLI-enum-01`：真实 CLI `bifrost remote traffic list --limit 10 --host example.com --from ... --to ...`；断言返回条目与 `args_json` 摘要一致。
- `TC-RI-回归-CLI-enum-02`：`bifrost remote traffic search --query "url~foo"`；断言 streaming 命令仍工作。
- `TC-RI-回归-CLI-enum-03`：`bifrost remote traffic get <id> --request-body --response-body`；断言 body 携带。

同步更新 `human_tests/readme.md` 索引与用例数量。

## Review/Fix/Test 闭环

### 第 1 轮

- 目标复核：`RemoteTrafficCommands` 尺寸问题彻底修复；CLI 表面 API 无差异。
- 代码 review：确认 `Box` 只在 CLI 层出现，`args_json` 序列化行为不变。
- 复测：`cargo clippy` + `cargo test -p bifrost-cli` + `remote traffic list` 真机。

### 第 2 轮

- 复核 `Search(Box<..>)` 是否也覆盖，避免第二次 `large-enum-variant`。
- 检查 `git diff` 有无遗漏的 `match` 分支或 `Box::new()` 缺失。
- 复测：完整 workspace 编译 + shell E2E。

## 校验要求

1. 先执行相关 E2E / 定向单元测试。
2. `cargo fmt --all -- --check`
3. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
4. `cargo test -p bifrost-cli`
5. `cargo test --workspace --all-features`
6. `bash scripts/ci/local-ci.sh --skip-e2e`

## 文档更新要求

- 本次改动不引入新 CLI 参数与用户可见能力，`README.md` 无需更新。
- `design/remote-traffic-cli-enum-size.md`（本文档）与 `human_tests/` 同步更新，满足仓库开发门禁。

## 风险与决策

- **Box 引入的一次堆分配**：每次调用增加 1 次堆分配，成本可忽略；换取 clippy 通过与栈占用下降。
- **序列化顺序**：`args_json` 字段顺序由 `serde_json::Map` / 手工构造决定，改动时需要保证与旧顺序一致，避免 relay 侧摘要 hash 抖动。
- **未来扩展**：新增 traffic 过滤参数直接加在 `RemoteTrafficListArgs` 内，无需再动 enum，避免复现同类问题。
- **同类 pattern**：`RemoteSearchArgs`、`RemoteRunArgs`、`RemoteCommandExecArgs` 已经使用 `Box`，本次改动与整体风格保持一致。
