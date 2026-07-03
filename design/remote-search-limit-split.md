# Remote Search 独立限制参数设计

> 状态：已交付并回归（2026-06-17 核对；文档层面仍存小尾巴，见文末 planned 清单） | 关联：`design/remote-command-isomorphic.md`、`design/search-jsonpath.md`

## 背景

`bifrost remote search` / `bifrost remote traffic search` 早期只暴露一个 `--limit` 参数，并在 caller 侧把它同时当成“输出条数提示”和“搜索接口 limit”传递。

但执行端 `SearchEngine` 实际有两套独立限制：

- `max_results`：最多返回多少条命中结果；
- `max_scan`：最多扫描多少条流量记录。

现状问题：

1. remote CLI 只暴露单个 `limit`，语义与本地 `bifrost search` 不一致；
2. remote invoke executor 仅向执行端传递 `max_results`；
3. 执行端搜索引擎仍按默认 `max_scan=100000` 继续扫描，导致 `--limit 2` 仍可能等待很久；
4. 用户会误以为 caller 没有限制生效。

## 用户目标验证清单

### 必须实现

- `bifrost remote search` 与 `bifrost remote traffic search` 都新增两个独立参数：
  - `--max-results`：最多返回多少条命中结果，默认 `100`；
  - `--max-scan`：最多扫描多少条记录，默认 `10000`。
- 参数在**执行端**生效（不是仅在 caller 输出层截断）。
- 保留 `--limit`（默认 `50`）作为向后兼容入口，caller 与执行端都能识别。
- caller `args_json` 三字段并存，执行端按 `max_results > legacy limit > 默认` 收敛。
- summary 输出同时展示 `max_results` 与 `max_scan`。

### 必须不破坏

- 本地 `bifrost search` 的参数设计不变。
- 现状 CLI 用户脚本仍可继续使用 `--limit`。
- 搜索引擎固定批量抓取策略保持原样。

### 必须真实验证

- 单测：
  - `crates/bifrost-cli/src/commands/remote.rs::test_build_remote_command_for_search`（`:7542`）与 `test_build_remote_command_for_traffic_search`（`:7635`）。
  - `crates/bifrost-admin/src/remote_invoke/executor.rs::test_search_stream_forwards_max_results_to_executor`（`~L4587`）与 `test_search_stream_forwards_max_scan_to_executor`（`:4637`）。
  - `crates/bifrost-cli/src/commands/remote.rs::print_remote_search_summary_plain_with_matches_and_more_flag`（`:11100`）与 `print_remote_search_summary_color_without_matches`（`:11105`）。
- E2E：`e2e-tests/tests/test_remote_invoke_e2e.sh` 的 TC-RI-04B / TC-RI-04E / `--limit 500 --max-results 500 --max-scan 2000` 组合。
- Human tests：`human_tests/remote-invoke.md` 的 TC-RI-86 / TC-RI-88 + 回归表 TC-RI-回归-128 / 133 / 135 / 136 / 137。

## 产品语义

搜索限制本质是两个独立门槛：

1. `max_scan` = 数据平面吃多少输入；决定搜索延迟与 CPU；
2. `max_results` = 结果平面吐多少输出；决定用户看到几条命中。

remote search 应把两者显式暴露给 caller，并透传到执行端。`--limit` 保留为兼容路径，语义等价于“旧版 `max_results` 提示”，与新参数**独立并存**（不是 clap 别名）。

## 技术细节

### 1. CLI 参数层（`crates/bifrost-cli/src/cli/remote.rs::RemoteSearchArgs`）

`RemoteSearchArgs` 同时被 `remote search` 与 `remote traffic search`（`RemoteTrafficCommands::Search(Box<RemoteSearchArgs>)`，`:1248`）共享，字段：

- `limit: usize`，`default_value = "50"`，help：`Maximum results to return`（保留兼容）。
- `max_results: Option<usize>`，`default_value = "100"`，help：`Maximum matching results to return (default: 100)`。
- `max_scan: Option<usize>`，`default_value = "10000"`，help：`Maximum records to scan (default: 10000, use larger value for broader search)`。

**实现偏差**：原计划「`--limit` 由 clap 统一映射到 `max_results`，避免双字段歧义」并未落地。三字段并存、并行进入 `args_json`。

### 2. caller → relay / openCall 参数透传（`crates/bifrost-cli/src/commands/remote.rs::build_remote_command`）

在 `search.get` / `traffic.search` 分支（`~L3000`）写入的 `args_json`（结构对应 `bifrost-command::SearchArgs`）至少包含：

- `keyword`（即 `query`，序列化字段名 `keyword`）
- `limit`（旧字段，等于 CLI `--limit`）
- `max_results`（新增，等于 CLI `--max-results`）
- `max_scan`（新增，等于 CLI `--max-scan`）
- `scope` / `filters` / `cursor` 等既有字段

实测：`test_build_remote_command_for_search`（`:7542`）与 `test_build_remote_command_for_traffic_search`（`:7635`）分别断言 `args.max_results` 与 `args.max_scan` 被同时序列化。`test_build_remote_command_for_search_supports_filter_only_query`（`:7707`）附加验证只带 filter 场景。

### 3. 执行端参数模型（`crates/bifrost-admin/src/remote_invoke/executor.rs`）

`search.*` 命令不复用 `executor.rs::CommandArgs`（该结构服务 traffic list/get 等），而是直接反序列化为 `bifrost-command::SearchArgs`（共享 crate `crates/bifrost-command/src/lib.rs:323`，含 `limit` / `max_results` / `max_scan` 三字段，互相独立）。

- `search_stream(query, legacy_limit, max_results, max_scan, on_stdout)`（`:3275 / 3279 / 3280`）接收三参数。
- 计算 `search_limit = max_results.or(legacy_limit).unwrap_or(50)`（`:3288`）。
- POST 到 `/_bifrost/api/search/stream`（`:3294`）：`payload = { "keyword": query, "max_results": search_limit, "max_scan": max_scan }`。
- 若走 in-process 分支：`search_stream_via_service`（`:3176`）内部同样透传三参数。
- 未提供 `max_scan` 时透传 `null`，由搜索接口自行使用默认值（`/_bifrost/api/search/stream` 默认 `max_scan=100000`）。

执行端行为：

- 未传 `max_results` 时回退到 `50`（与文档一致）。
- 未传 `max_scan` 时透传 `null`，让搜索接口决定默认。
- summary 由 `print_remote_search_summary(keyword, max_scan, max_results, ...)`（`:4634`）渲染；`max_results` 缺省回退到 `100`、`max_scan` 缺省回退到 `0`（语义为“未限制”）。

### 4. 兼容性

- 旧 caller 仍发送 `limit`（共享 `SearchArgs` 中字段未删除），新 caller 在 `args_json` 中同时携带 `limit` / `max_results` / `max_scan`。
- 执行端 `search_stream` 实际优先级：`max_results > legacy limit > 默认 50`。
- `max_scan` 直接透传，未提供任何回退默认值（由下游搜索接口决定）。
- E2E 已验证调用记录可见 `max_results` / `max_scan`（TC-RI-04B / TC-RI-04E）。

## CLI + Web + Admin API

- CLI：新增 `--max-results` / `--max-scan`，help 明确区分“返回命中数”与“扫描记录数”；`--limit` help 自身未显式标注“向后兼容 / 优先 `--max-results`”（planned, not yet shipped as of 2026-06-17）。
- Web：Remote 面板对应输入框可后续扩展；本轮不改。
- Admin API：`/_bifrost/api/search/stream` 已长期接受 `max_results` + `max_scan`，无需变更；变化仅在 remote invoke executor 与 caller 之间。

## Sync 边界

搜索限制是每次 CLI 调用的临时参数，不进入本地或 relay 的持久化存储；无 sync 影响。

## Phase 拆分

### Phase 1：共享 SearchArgs 三字段

- `bifrost-command::SearchArgs` 增加 `max_results` / `max_scan`（`limit` 保留）。

### Phase 2：CLI 参数暴露

- `RemoteSearchArgs` 增加 `--max-results` / `--max-scan`。
- `remote search` 与 `remote traffic search` 共享同一结构。

### Phase 3：executor 透传

- `search_stream` 接收三参数并计算 `search_limit`；
- summary 输出两组数字。

### Phase 4：测试 + E2E + human_tests

- caller 序列化断言。
- executor mock 断言 POST body 含 `max_results` / `max_scan`。
- E2E TC-RI-04B / 04E；human_tests TC-RI-86 / 88。

## 测试方案

### 单元测试

- `crates/bifrost-cli/src/commands/remote.rs::test_build_remote_command_for_search`（`:7542`）：断言 `args.max_results=Some(7)`、`args.max_scan=Some(12)` 同时携带在 `args_json` 中。
- `test_build_remote_command_for_traffic_search`（`:7635`）：断言 `args.max_results=Some(3)`、`args.max_scan=Some(9)` 与 `args.limit=Some(5)` 三字段并存。
- `test_build_remote_command_for_search_supports_filter_only_query`（`:7707`）：仅带 `--filter` 时序列化正确。
- `crates/bifrost-admin/src/remote_invoke/executor.rs::test_search_stream_formats_incremental_output`（`:4587`）：POST body `"max_results":5`。
- `test_search_stream_forwards_max_scan_to_executor`（`:4637`）：POST body 包含 `"max_scan":20`。
- `print_remote_search_summary_plain_with_matches_and_more_flag`（`:11100`）与 `print_remote_search_summary_color_without_matches`（`:11105`）：summary 渲染两组阈值。
- 「旧 caller 仍可被识别」专用回归用例（planned, not yet shipped as of 2026-06-17）。当前依赖 `bifrost-command::SearchArgs` 字段默认值间接覆盖。

### E2E 测试

`e2e-tests/tests/test_remote_invoke_e2e.sh`：

- TC-RI-04B：`bifrost remote search <marker> ... --max-results 5 --max-scan 50`，断言 `args_json` 包含 `"max_results":5` 与 `"max_scan":50`。
- TC-RI-04E：`bifrost remote traffic search <marker> ... --max-results 3 --max-scan 30`，断言 `args_json` 同时携带 `keyword/query` 与新参数。
- `--limit 500 --max-results 500 --max-scan 2000` 组合：验证三参数互不干扰。
- 相邻 `test_remote_invoke_recent_calls_args_preview_e2e.sh` 复验 args preview 中三字段可见。

### 真实场景测试（`human_tests/remote-invoke.md`）

- TC-RI-86：`remote search ... --max-results 5 --max-scan 50`，验证 `search.get` 调用参数包含 `max_results=5` / `max_scan=50`。
- TC-RI-88：`remote traffic search ... --max-results 3 --max-scan 30`，验证 `command.args_json` 同时包含 `query`、`max_results=3`、`max_scan=30`。
- 回归表：TC-RI-回归-128 / 133 / 135 / 136 / 137 多次复验 `command_summary.masked_args_json` 可见 `keyword/max_results/max_scan`。
- `human_tests/readme.md` 索引层面显式列出 TC-RI-86 / 88（planned, not yet shipped as of 2026-06-17）。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核 `RemoteSearchArgs` 三字段的 default 与 help，确保 CLI --help 输出可读、可测试。
- 复核 caller 序列化 `args_json` 三字段并存。
- 复测：单测 + E2E TC-RI-04B / 04E。

### 第 2 轮

- 复核 executor `search_stream` fallback 优先级（`max_results > limit > 50`）。
- 复核 summary 输出 `max_scan=0` 时的“未限制”文案。
- 复测：`human_tests/remote-invoke.md` TC-RI-86 / 88 手动执行。

## 风险与决策

| 风险 | 缓解 |
|---|---|
| 双字段并存造成语义困惑 | help 文案区分「返回命中数」vs「扫描记录数」；文档规范推荐使用 `--max-results` / `--max-scan` |
| `max_scan=null` 让搜索长时间跑满 100000 | CLI 默认 `--max-scan=10000`，仅在用户主动传 `--max-scan 0` 或删掉参数时才落到 100000 |
| 未来去除 `--limit` 会破坏脚本 | 保留 legacy 字段作为兼容入口；改期需要独立设计文档 |
| E2E 用 `5/50`、`3/30` 而非设计示例 `2/20` | 断言逻辑等价，未来若统一示例需要同步 fixture |

## 校验要求

- 先执行与本改动相关的 E2E 测试：`bash e2e-tests/tests/test_remote_invoke_e2e.sh`。
- 再执行 `cargo fmt --all -- --check`。
- 再执行 `cargo clippy --workspace --all-targets --all-features -- -D warnings`。
- 至少执行一次 `cargo test --workspace --all-features`。
- 最后执行仓库要求的 rust-project-validate 流程。

## 文档更新要求

- 更新 `docs/cli.md` 中 remote search / remote traffic search 参数说明（planned, not yet shipped as of 2026-06-17）——当前 `docs/cli.md` 仍未出现 `--max-results` / `--max-scan` 文档化条目；CLI help 文案已包含新参数说明，需把它们补进 `docs/cli.md` 与 docs-en 镜像。
- 若 CLI help 文案变化，同步保证示例命令与文档一致（受上一条阻塞）。
- `human_tests/readme.md` 索引显式列出 TC-RI-86 / 88（planned, not yet shipped as of 2026-06-17）。
