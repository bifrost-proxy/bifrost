# Remote Search 独立限制参数设计

## 背景

`bifrost remote search` / `bifrost remote traffic search` 当前只暴露一个 `--limit` 参数，并在 caller 侧把它同时当成“输出条数提示”和“搜索接口 limit”传递。

但执行端 `SearchEngine` 实际有两套独立限制：

- `max_results`：最多返回多少条命中结果
- `max_scan`：最多扫描多少条流量记录

现状问题：

1. remote CLI 只暴露单个 `limit`，语义与本地 `bifrost search` 不一致
2. remote invoke executor 仅向执行端传递 `max_results`
3. 执行端搜索引擎仍会按默认 `max_scan=100000` 继续扫描，导致 `--limit 2` 仍可能等待很久
4. 用户会误以为 caller 没有限制生效

## 目标

为 remote search 提供与本地搜索一致的两类独立限制，并确保限制在**执行端**生效：

- `--max-results`：控制最多返回多少条命中结果
- `--max-scan`：控制最多扫描多少条记录

同时保留现有 `--limit` 仅作为向后兼容别名，语义收敛为：

- `--limit` 等价于 `--max-results`

## 非目标

- 不修改本地 `bifrost search` 的参数设计
- 不把限制只做在 caller 输出层
- 不调整搜索引擎固定批量抓取策略以外的整体搜索架构

## 实现方案

### 1. CLI 参数层

更新 `crates/bifrost-cli/src/cli.rs`：

- `remote search`
  - 新增 `--max-results`
  - 新增 `--max-scan`
  - `--limit` 保留为 `max_results` 的别名
- `remote traffic search`
  - 同样新增 `--max-results`
  - 同样新增 `--max-scan`
  - `--limit` 保留为 `max_results` 的别名

约束：

- 若用户同时传 `--limit` 与 `--max-results`，clap 统一映射到同一字段，避免双字段歧义
- help 文案明确区分“返回命中数”和“扫描记录数”

### 2. caller -> relay/openCall 参数透传

更新 `crates/bifrost-cli/src/commands/remote.rs`：

- `build_remote_command()` 为 `search.get` / `traffic.search` 构造 `args_json` 时写入：
  - `query`
  - `max_results`
  - `max_scan`

不再仅发送 `limit`

### 3. 执行端参数模型

更新 `crates/bifrost-admin/src/remote_invoke/executor.rs`：

- `CommandArgs` 新增：
  - `max_results: Option<usize>`
  - `max_scan: Option<usize>`
- `search_stream()` 改为接收两个独立参数
- 发给 `/_bifrost/api/search/stream` 的 JSON payload 透传：
  - `max_results`
  - `max_scan`

执行端行为要求：

- 若未传 `max_results`，沿用当前 remote search 默认 `50`
- 若未传 `max_scan`，执行端使用搜索接口默认值
- summary 中展示的 `limit` 改为使用实际 `max_results`

### 4. 兼容性

- 旧 caller 仍可能发送 `limit`
- 执行端为了兼容旧版本，可继续保留 `limit` 反序列化字段，但新代码优先使用 `max_results`
- 新 caller 发起的调用记录中应能看到 `max_results` / `max_scan`

## 测试方案

### 单元测试

- `build_remote_command_for_search`：验证 `search.get` args_json 包含 `max_results` 与 `max_scan`
- `build_remote_command_for_traffic_search`：验证 `traffic.search` args_json 包含 `max_results` 与 `max_scan`
- `remote_invoke executor`：验证旧 `limit` 兼容，新 `max_results` / `max_scan` 可反序列化

### E2E 测试

更新 `e2e-tests/tests/test_remote_invoke_e2e.sh`：

- 新增 remote search 参数透传验证
- 调用 `remote search <marker> --max-results 2 --max-scan 20`
- 断言 Recent Calls / call detail 中 `args_json` 包含 `max_results=2`、`max_scan=20`
- 断言调用成功结束，说明执行端接受了新参数

### 真实场景测试（human_tests）

更新 `human_tests/remote-invoke.md`：

- 新增回归用例，验证：
  - `--max-results` 只控制返回结果数
  - `--max-scan` 由执行端限制扫描范围
  - 调用记录里能看到透传后的参数
- 同步更新 `human_tests/readme.md`

## 校验要求

- 先执行与本改动相关的 E2E 测试
- 再执行 `cargo fmt --all -- --check`
- 再执行 `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- 至少执行一次 `cargo test --workspace --all-features`
- 最后执行仓库要求的 rust-project-validate 流程

## 文档更新要求

- 更新 `docs/cli.md` 中 remote search / remote traffic search 参数说明
- 若 CLI help 文案变化，同步保证示例命令与文档一致
