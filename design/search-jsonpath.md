# Search JSONPath / Header / Time-window 过滤与 JSON 输出

## 背景

老版 `FilterCondition` 只支持 compact 字段（`url|host|path|method|content_type|client_app|client_ip|listener_port`）加上 SQL side 的 `status_ranges/protocols/content_types` 粗粒度分组。诊断 LLM/网关流量的常见诉求：

- 按响应 body 中的 JSON 字段过滤：`errno`、`choices[0].finish_reason`、`error.message`。
- 按 header 过滤：`Authorization`、`Set-Cookie`、`x-trace-id`。
- 按时间窗收敛：只看最近 5 分钟 5xx、只看 08:00–09:00 的调用。
- 拿到结构化输出：脚本 / ai-report 不想解析 SSE 或 CLI table。

本设计扩展 `FilterCondition.field` 语义、新增 `time_range`、新增 CLI 标志与 JSON/NDJSON 输出，不破坏 SSE / SQL / body 落盘格式。

## 用户目标验证清单

### 必须实现

- `FilterCondition.field` 新增四类前缀：
  - `req.body.$.<jsonpath>` / `res.body.$.<jsonpath>`
  - `req.header.<name>` / `res.header.<name>`（header 名大小写不敏感）
  - `ts`（unix epoch ms）
- 新增 operator：`lt|gt|lte|gte`（仅对 `ts` 和数字型 JSON path 值生效）。
- `SearchRequest` 新增 `time_range { since_ms, until_ms }`，SQL 层预剪枝。
- `SearchResponse` 新增 `searched_range { oldest_ts_ms, newest_ts_ms, scanned_count }`。
- CLI 新增标志：
  - `--req-json path=value`（可重复）、`--res-json path=value`（可重复）
  - `--req-header-eq name=val`（可重复）、`--res-header-eq name=val`（可重复）
  - `--req-header name` / `--res-header name`（只在对应侧搜索匹配）
  - `--since <duration>` / `--until <duration>` / `--latest`
  - `--format json|json-pretty|ndjson`
- `bifrost remote traffic search` 同步这批标志。

### 必须不破坏

- 老 `FilterCondition` 分支保持不变；未知 field 走新分支。
- 老服务端遇到带 `time_range` 的请求时 `serde` 按 `Option::default()` 处理。
- 老 CLI 解析新 `searched_range` 时按未知字段忽略。
- SSE search 协议、SQL schema、`BodyStore` 落盘格式一律不动。

### 必须真实验证

- `bifrost search --res-json '$.errno=90000201'` 命中包含错误码的响应。
- `bifrost search --res-header 'set-cookie' --since 5m` 命中最近 5 分钟设置 Cookie 的响应。
- `--until <过去时间>` 走 SQL 剪枝，`searched_range.scanned_count == 0`。
- `--latest` 等价 `--limit 1 --max-results 1`。

## 产品语义

### JSONPath 子集

`crates/bifrost-admin/src/search/json_path.rs`，纯 `std + serde_json`：

- 语法：`$(\.<key>|\[<idx>\]|\[\*\])*`
- 语法元素：
  - `$` 根
  - `.foo` 对象成员
  - `[0]` / `[12]` 非负整数下标
  - `[*]` 数组通配，发散所有子元素
- **不支持** 过滤表达式 `?(@.foo>1)`、递归通配 `..`、括号切片。
- 路径解析失败 → 返回空 `Vec`，调用方按“不匹配”处理。

### FilterCondition 求值语义

- **文本类** operator（`contains|equals|not_contains|is_empty|is_not_empty|regex`）：
  - 命中值先 stringify：标量 `to_string`，复杂对象 `Value::to_string`。
  - `regex` 走 `Regex::is_match`。
- **数字类** operator（`lt|gt|lte|gte`）：
  - 值尝试 `as_f64`，`condition.value.parse::<f64>()`。
  - 类型不匹配视为不命中。
- Header：`fields.headers` lower-case 匹配，多值任一命中即命中；`fields = None` 时 false。
- `ts`：`compact.ts` vs `condition.value.parse::<i64>()`；只支持 `lt|gt|lte|gte`，其他 operator 视为不命中。

### time_range 预剪枝

`SearchRequest.time_range.since_ms/until_ms` 下推到 `QueryParams.since_ms/until_ms`，SQL `WHERE timestamp >= ? AND timestamp <= ?` 直接筛。SQL 剪掉的记录：
- 不进入 SearchEngine matcher。
- 不消耗 `max_scan`。
- 不触发 body/header 解析。
- 空窗口 `searched_range.scanned_count == 0`。

### body cache 单次搜索复用

`SearchEngine` per-request 引入：

```rust
enum BodyCacheEntry {
    Json(Value),
    NonJson,
    Missing,
}
let mut body_cache: HashMap<String, BodyCacheEntry> = HashMap::new();
```

同一记录的同侧 body 只解析一次。JSON 解析失败标记 `NonJson` 不重试。

## 技术细节

### 类型

`crates/bifrost-admin/src/search/types.rs`：

```rust
pub struct SearchRequest {
    // ...
    pub time_range: Option<TimeRange>,
    // ...
}

pub struct TimeRange {
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
}

pub struct SearchResponse {
    // ...
    pub searched_range: SearchedRange,
}

pub struct SearchedRange {
    pub oldest_ts_ms: Option<i64>,
    pub newest_ts_ms: Option<i64>,
    pub scanned_count: usize,
}
```

### Engine 求值

`crates/bifrost-admin/src/search/engine.rs`：

```rust
let cond_needs_req_header = conds.iter().any(|c| c.field.starts_with("req.header."));
let cond_needs_res_header = conds.iter().any(|c| c.field.starts_with("res.header."));
let cond_needs_req_body   = conds.iter().any(|c| c.field.starts_with("req.body."));
let cond_needs_res_body   = conds.iter().any(|c| c.field.starts_with("res.body."));

// per-request body cache
let mut body_cache: HashMap<String, BodyCacheEntry> = HashMap::new();

// match 分支
} else if let Some(rest) = field.strip_prefix("req.header.") {
    // fields.headers.request lower-case match
} else if let Some(path) = field.strip_prefix("req.body.") {
    Self::eval_body_path(fields, path, condition, BodySide::Req, body_cache)
} else if let Some(path) = field.strip_prefix("res.body.") {
    Self::eval_body_path(fields, path, condition, BodySide::Res, body_cache)
}
```

- `eval_body_path` 拉 `BodyStore` 内容，`serde_json::from_str`，`json_path::eval(&value, path)`。
- 结果 nodes 迭代比较；任一命中即整条命中。

### CLI

`crates/bifrost-cli/src/cli.rs` Search 子命令：

- `--req-json <path=value>`（可重复）
- `--res-json <path=value>`（可重复）
- `--req-header-eq <name=value>`（可重复）
- `--res-header-eq <name=value>`（可重复）
- `--req-header <name>` / `--res-header <name>`：只在该侧搜索关键词。
- `--since <duration>` / `--until <duration>`：
  - `30s/5m/2h/1d`：相对 `now`。
  - RFC3339（如 `2026-06-17T10:00:00Z`）：绝对时间。
- `--latest`：等价 `--limit 1 --max-results 1`。
- `OutputFormat` 新增 `Ndjson`（保留 `Json/JsonPretty/Table/Compact`）。

`run_simple_search` 分支：
- `json/json-pretty/ndjson`：专门 collector；聚合 `results` + `searched_range` + `time_range` 整体/逐条输出。
- SSE `done` 事件必须携带 `searched_range`，CLI 结构化输出能验证 SQL 预剪枝没有消耗 `max_scan`。

### 远端

`crates/bifrost-cli/src/cli/remote.rs` `RemoteSearchArgs` 同步这批标志。`crates/bifrost-cli/src/commands/remote.rs` 的 `command_search_args` 把它们映射成 `FilterCondition` + `time_range`。

## CLI / Web / Admin API 快照

| 层 | 入口 | 能力 |
|---|---|---|
| CLI | `bifrost search --req-json/--res-json/...` | body JSONPath 过滤 |
| CLI | `bifrost search --req-header-eq/--res-header-eq` | header 精确过滤 |
| CLI | `bifrost search --since/--until/--latest` | 时间窗与最新一条 |
| CLI | `bifrost search --format json\|json-pretty\|ndjson` | 结构化输出 |
| CLI | `bifrost remote traffic search ...` | 远端同能力 |
| Admin API | `POST /api/traffic/search` | `time_range` + 新 field 语义 |
| Admin API | SSE `done` 事件 | 携带 `searched_range` |
| Web | Traffic Search 面板 | 前端可选接入 header/body 高级过滤（本设计不改前端 UI 结构） |

## Sync 边界

Traffic 存本地 SQLite，`search` 是本地/远端 admin API，不与 sync 交互。远端调用走 `bifrost remote invoke`。

## Phase 拆分

- **Phase 1**：`json_path.rs` 子集 + 单测；`FilterCondition` 新 field 分支。
- **Phase 2**：`time_range` + SQL 剪枝 + `searched_range` 输出；body cache 复用。
- **Phase 3**：CLI 标志（`--req-json/--res-json/--req-header-eq/--res-header-eq/--since/--until/--latest`）+ `--format ndjson`；SSE done 携带 `searched_range`。
- **Phase 4**：`bifrost remote traffic search` 参数同步 + `command_search_args` 映射；E2E + human_tests。

## 测试方案

### 单元测试

- `crates/bifrost-admin/src/search/json_path.rs`：10 个 `#[test]`（基本嵌套、数组下标、通配、缺失字段、根 `$`、空数组通配、边界）。
- `crates/bifrost-admin/src/search/engine.rs`：
  - `json_path_req_body_filter_matches`
  - `json_path_res_body_numeric_gt_filter`
  - `req_header_x_trace_id_filter`
  - `time_range_prunes_out_of_window_records`
  - `body_cache_reused_across_conditions`
  - `non_json_body_is_marked_and_skipped`
- `crates/bifrost-cli`：
  - duration 解析 4 个：`5m`、`2h`、RFC3339、非法。
  - `command_search_args` 映射 `FilterCondition` + `time_range`。

### E2E 测试

- `e2e-tests/tests/test_search_traffic_cli_isomorphic_e2e.sh`：
  - `--until` 早于所有记录：`searched_range.scanned_count == 0`。
  - `--res-json '$.errno=90000201'` 命中且 CLI JSON 输出结构化 `results + searched_range`。
  - `--latest` 只回一条。

### 真实场景测试 human_tests

`human_tests/search-jsonpath.md`：

- TC-SJP-01：`bifrost search --since 5m --format json-pretty` 观察 `searched_range`。
- TC-SJP-02：`bifrost search --until 2026-01-01T00:00:00Z` 空窗口 `scanned_count == 0`。
- TC-SJP-03：`bifrost search --latest --format json` 只输出一条。
- TC-SJP-04：`bifrost search --req-json '$.messages[0].role=user'` 命中 LLM 请求。
- TC-SJP-05：`bifrost search --res-header-eq 'set-cookie=session=abc'` 命中特定 Cookie。
- TC-SJP-06：混合 `--since 1h --res-json '$.error.message*=timeout' --format ndjson`。

### 覆盖率与项目校验

- `cargo test -p bifrost-admin json_path`
- `cargo test -p bifrost-admin -p bifrost-cli -p bifrost-command`
- `cargo fmt --all -- --check`、clippy workspace 全绿
- 本地不跑 coverage，交给远端 CI

## Review / Fix / Test 闭环

### 第 1 轮

- 复核 `FilterCondition` 未知 field 走新分支不影响旧 filter。
- 复核 `time_range` 空窗口 SQL 剪枝彻底：不解析 body、不消耗 max_scan。
- 复核 body cache 只在单次 `search_internal` 生存，跨请求不复用。

### 第 2 轮

- 复核 CLI 参数别名与文档一致；`--since/--until` 支持 duration 与 RFC3339。
- 复核 SSE done 事件 payload 结构（`searched_range` 存在且字段稳定）。
- 复跑 E2E 与 human_tests，包括远端 `bifrost remote traffic search`。

## 风险与决策

- **只做 JSONPath 子集**：`?(@.foo>1)` 与递归 `..` 语义复杂，容易踩解析器坑；第一版明确只做常见路径，覆盖 90% LLM/网关诊断需求。
- **数字比较容错**：`lt/gt/lte/gte` 只对 `f64` 可解析值生效，避免字符串比较歧义；文档要提醒 `errno` 之类字符串数字要用 `equals` 而不是 `gt`。
- **body cache 命中率**：单次搜索复用足够；跨 search 复用会引入内存和一致性问题，第一版不做。
- **`searched_range` 语义边界**：只表示实际扫描过的记录范围，不代表 SQL 全量记录；空窗口场景 `scanned_count == 0` 是明确信号。
- **性能压力**：`--req-json/--res-json` 会拉 body，超大 body 会拖慢，建议搭配 `time_range`、`--limit` 收敛。
