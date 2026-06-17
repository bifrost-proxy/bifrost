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

同时保留现有 `--limit` 字段（仍在 `RemoteSearchArgs` 和 `bifrost-command::SearchArgs` 中）作为向后兼容入口，新的 `--max-results` / `--max-scan` 由独立 clap 字段承载，并行透传到执行端。

说明（2026-06-17 实现差异）：`--limit` 与 `--max-results` 实际是 **两个独立 clap 字段**，并未通过 clap 互为别名映射到同一字段（参见 `crates/bifrost-cli/src/cli/remote.rs::RemoteSearchArgs`：`limit: usize`（默认 50）与 `max_results: Option<usize>`（默认 100））。caller 把三者一起塞进 `args_json`，由执行端按 `max_results.or(legacy_limit).unwrap_or(50)` 的优先级使用（参见 `crates/bifrost-admin/src/remote_invoke/executor.rs::search_stream`）。

## 非目标

- 不修改本地 `bifrost search` 的参数设计
- 不把限制只做在 caller 输出层
- 不调整搜索引擎固定批量抓取策略以外的整体搜索架构

## 实现方案

### 1. CLI 参数层

现状（实现在 `crates/bifrost-cli/src/cli/remote.rs::RemoteSearchArgs`，同时被 `remote search` 与 `remote traffic search`（`RemoteTrafficCommands::Search(Box<RemoteSearchArgs>)`）共享）：

- `remote search` / `remote traffic search` 都已新增：
  - `--max-scan`（`Option<usize>`，`default_value = "10000"`，help：`Maximum records to scan (default: 10000, use larger value for broader search)`）
  - `--max-results`（`Option<usize>`，`default_value = "100"`，help：`Maximum matching results to return (default: 100)`）
- 旧 `--limit`（`usize`，`default_value = "50"`，help：`Maximum results to return`）**仍以独立字段保留**，并未被 clap 折叠成 `max_results` 的别名。三个字段并存、并行进入 `args_json`。

实现偏差（planned, not yet shipped as of 2026-06-17）：

- 原计划「`--limit` 由 clap 统一映射到 `max_results`，避免双字段歧义」并未落地。caller 仍发送 `{limit, max_results, max_scan}` 三元组，由执行端按优先级处理。
- help 文案区分「返回命中数」与「扫描记录数」已实现；`--limit` 自身 help 未显式标注「向后兼容/优先 `--max-results`」（planned, not yet shipped as of 2026-06-17）。

### 2. caller -> relay/openCall 参数透传

实现位于 `crates/bifrost-cli/src/commands/remote.rs::build_remote_command`（具体 `SearchArgs` 构造分支位于 `~line 3000`）。`search.get` / `traffic.search` 现在写入的 `args_json`（结构对应 `bifrost-command::SearchArgs`）至少包含：

- `keyword`（即 `query`，对应 `SearchArgs::keyword`，序列化字段名 `keyword`）
- `limit`（旧字段，兼容传输，等于 CLI `--limit`）
- `max_results`（新增，等于 CLI `--max-results`）
- `max_scan`（新增，等于 CLI `--max-scan`）
- 以及 `scope` / `filters` / `cursor` 等既有字段

实测覆盖见 `crates/bifrost-cli/src/commands/remote.rs` 中 `test_build_remote_command_for_search` / `test_build_remote_command_for_traffic_search`，断言 `args.max_results` 与 `args.max_scan` 同时被序列化。注意：完全去除 `limit` 字段属于后续清理动作（planned, not yet shipped as of 2026-06-17）。

### 3. 执行端参数模型

实现位于 `crates/bifrost-admin/src/remote_invoke/executor.rs`。注意：search.* 命令并不复用 `executor.rs::CommandArgs`（该结构服务于 traffic/list/get 等命令），而是直接反序列化为 `bifrost-command::SearchArgs`（已在共享 crate 中包含 `limit` / `max_results` / `max_scan` 三字段，互相独立）。

现状：

- `search_stream(query, legacy_limit, max_results, max_scan, on_stdout)` 已接收三参数（`crates/bifrost-admin/src/remote_invoke/executor.rs:3181~3185`）。
- 计算 `search_limit = max_results.or(legacy_limit).unwrap_or(50)`，并通过 `payload = {"keyword": query, "max_results": search_limit, "max_scan": max_scan}` POST 到 `/_bifrost/api/search/stream`（同文件 ~line 3192-3200）。
- 单元测试覆盖：`test_search_stream_forwards_max_results_to_executor`、`test_search_stream_forwards_max_scan_to_executor`（同文件 ~line 4493 / 4534）。

执行端行为要求落地状态：

- 未传 `max_results` 时回退到 `50`（与文档一致）。
- 未传 `max_scan` 时透传 `null` 给搜索接口，由搜索接口自行使用默认值（`/_bifrost/api/search/stream` 默认 `max_scan=100000`，参考背景描述）。
- summary 由 `print_remote_search_summary(keyword, max_scan, max_results, ...)` 渲染（`crates/bifrost-cli/src/commands/remote.rs::print_remote_search_summary`，~line 3442）。`max_results` 缺省回退到 `100`、`max_scan` 缺省回退到 `0`（语义为「未限制」）。

### 4. 兼容性

- 旧 caller 仍发送 `limit`（共享 `SearchArgs` 中字段未删除），新 caller 在 `args_json` 中同时携带 `limit` / `max_results` / `max_scan`，与现状一致。
- 执行端 `search_stream` 实际优先级：`max_results > legacy limit > 默认 50`；`max_scan` 直接透传，未提供任何回退默认值（由下游搜索接口决定）。
- E2E 已验证调用记录可见 `max_results` / `max_scan`：见 `e2e-tests/tests/test_remote_invoke_e2e.sh` TC-RI-04B / TC-RI-04E（断言 `args_json` 包含 `"max_results":5` 与 `"max_scan":50` 等）。

## 测试方案

### 单元测试

现状：

- `crates/bifrost-cli/src/commands/remote.rs::test_build_remote_command_for_search`：已断言 `args.max_results=Some(7)`、`args.max_scan=Some(12)` 同时携带在 `args_json` 中。
- 同文件 `test_build_remote_command_for_traffic_search`：已断言 `args.max_results=Some(3)`、`args.max_scan=Some(9)` 与 `args.limit=Some(5)` 三字段并存。
- `crates/bifrost-admin/src/remote_invoke/executor.rs::test_search_stream_forwards_max_results_to_executor` / `test_search_stream_forwards_max_scan_to_executor`：mock `/_bifrost/api/search/stream`，断言 POST body 含 `"max_results":5` 与 `"max_scan":20`。
- 旧 `limit` 反序列化兼容由共享 `bifrost-command::SearchArgs` 字段默认值保证；专用「旧 caller 仍可被识别」回归用例（planned, not yet shipped as of 2026-06-17）。

### E2E 测试

现状：`e2e-tests/tests/test_remote_invoke_e2e.sh` 已新增以下用例：

- TC-RI-04B：`bifrost remote search <marker> ... --max-results 5 --max-scan 50`，断言 `args_json` 包含 `"max_results":5` 与 `"max_scan":50`。
- TC-RI-04E：`bifrost remote traffic search <marker> ... --max-results 3 --max-scan 30`，断言 `args_json` 同时携带 `keyword/query` 与新参数。
- 另有 `--limit 500 --max-results 500 --max-scan 2000` 组合作为取消用例（验证三参数互不干扰）。

参数采用 `5/50` / `3/30` 而非设计中举例的 `2/20`，断言逻辑等价。

### 真实场景测试（human_tests）

现状：`human_tests/remote-invoke.md` 已新增 TC-RI-86 / TC-RI-88：

- TC-RI-86：`remote search ... --max-results 5 --max-scan 50`，验证 `search.get` 调用参数包含 `max_results=5` / `max_scan=50`。
- TC-RI-88：`remote traffic search ... --max-results 3 --max-scan 30`，验证 `command.args_json` 同时包含 `query`、`max_results=3`、`max_scan=30`。
- 回归记录表（TC-RI-回归-128 / 133 / 135 / 136 / 137 等）已多次复验 `command_summary.masked_args_json` 中可见 `keyword/max_results/max_scan`。

`human_tests/readme.md` 同步更新（planned, not yet shipped as of 2026-06-17）—当前未在 readme 索引中显式列出 TC-RI-86 / 88，但章节已存在于 remote-invoke.md。

## 校验要求

- 先执行与本改动相关的 E2E 测试
- 再执行 `cargo fmt --all -- --check`
- 再执行 `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- 至少执行一次 `cargo test --workspace --all-features`
- 最后执行仓库要求的 rust-project-validate 流程

## 文档更新要求

- 更新 `docs/cli.md` 中 remote search / remote traffic search 参数说明（planned, not yet shipped as of 2026-06-17）—当前 `docs/cli.md` 仍未出现 `--max-results` / `--max-scan` 文档化条目；CLI help 文案已包含新参数说明，需把它们补进 docs/cli.md 与 docs-en 镜像。
- 若 CLI help 文案变化，同步保证示例命令与文档一致（受上一条阻塞）。
