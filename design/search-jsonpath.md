# Search JSONPath / Header / Time-window 过滤与 JSON 输出

## 背景

 现有 `FilterCondition` 仅支持 `url|host|path|method|content_type|client_app|client_ip|listener_port` 等 compact 字段，外加 SQL-side `status_ranges/protocols/content_types` 等粗粒度过滤。诊断 LLM/网关流量时常需按 body 中的 JSON 字段（`errno` / `choices[0].finish_reason` / `error.message`）或 header（`Authorization` / `Set-Cookie` / 自定义 trace-id）精确过滤，并按时间窗收敛。同时脚本调用方希望直接拿结构化输出，免去解析 SSE/table。本设计在不破坏现有协议的前提下，扩展 `FilterCondition.field` 语义、新增 `time_range`、新增 CLI 标志与 JSON/NDJSON 输出。

## 目标

1. `FilterCondition.field` 增加四类前缀语法：
   - `req.body.$.<jsonpath>` / `res.body.$.<jsonpath>`
   - `req.header.<name>` / `res.header.<name>`（header 名大小写不敏感）
   - `ts`（unix epoch ms）
2. `operator` 在现有 `contains|equals|not_contains|is_empty|is_not_empty|regex` 基础上新增 `lt|gt|lte|gte`，仅对 `ts` 和数字型 JSON path 值生效。
3. `SearchRequest` 新增 `time_range { since_ms, until_ms }`：与 `ts` 条件功能等价但独立字段，便于底层提前剪枝。
4. `SearchResponse` 新增 `searched_range { oldest_ts_ms, newest_ts_ms, scanned_count }`，方便调用方了解扫描覆盖。
5. CLI：新增 `--req-json k=v`、`--res-json k=v`、`--req-header name=val`、`--res-header name=val`、`--since`、`--until`、`--latest`、`--format json|ndjson`。

## 非目标

- 不实现 JSONPath 过滤表达式（`?(@.foo>1)`）。
- 不实现递归通配 `..`。
- 不改 SSE 协议、不改 SQL schema、不改 BodyStore 落盘格式。
- 不破坏既有 flag/默认行为。

## JSONPath 子集

新增 `crates/bifrost-admin/src/search/json_path.rs`，纯 `std + serde_json`：

- 语法：`$(\.<key>|\[<idx>\]|\[\*\])*`
  - `$`：根
  - `.foo`：对象成员
  - `[0]` / `[12]`：数组下标（非负整数）
  - `[*]`：数组通配，发散为所有子元素
- API：`pub fn eval(value: &Value, path: &str) -> Vec<&Value>`
- 错误（路径解析失败）→ 返回空 `Vec`，调用方按"不匹配"处理。
- 单测覆盖：基本嵌套、数组下标、通配命中、缺失字段、根 `$`、空数组通配。

## FilterCondition 求值

在 `SearchEngine` 引入 per-request `body_cache: HashMap<String, BodyCacheEntry>`，避免同一记录多次解析。

```
enum BodyCacheEntry { Json(Value), NonJson, Missing }
```

- `req.body.$.x` / `res.body.$.x`：
  - 从 `fields` 取 `request_body_ref` / `derived_response_body_ref.or(response_body_ref)`；
  - 通过 `BodyStore` 读取字符串，`serde_json::from_str` 解析；
  - 调 `json_path::eval(&value, &condition.field["req.body.".len()..])`；
  - 对每个命中值与 `condition.value` 比较：
    - `contains/equals/not_contains/is_empty/is_not_empty`：先把值序列化为 `stringify`（标量直接 `to_string`，复杂对象 `Value::to_string`），再走文本比较；
    - `lt/gt/lte/gte`：尝试 `as_f64` 与 `condition.value.parse::<f64>()` 比较；
    - `regex`：对 stringify 后字符串 `Regex::is_match`；
  - 命中规则："任一命中即整条命中"。
- `req.header.X` / `res.header.X`：从 `fields` headers 中按 header name lowercase 匹配；多值（同名重复 header）任一命中即命中。fields 为 None 时返回 `false`。
- `ts`：`compact.ts` 与 `condition.value.parse::<i64>()` 比较，必须用 `lt|gt|lte|gte`，其他 operator 视为不命中。

## time_range

- `SearchRequest.time_range.since_ms` / `until_ms` 在 `matches_filter_compact` 中实现：`ts < since` 或 `ts > until` 直接 false。
- 不下推到 SQL（避免改 QueryParams schema），但可避免对剩余 record 的 body/header 解析。

## SearchResponse.searched_range

- 在迭代过程中跟踪 `min/max ts`、`scanned_count`，最后一并写入。
- 序列化：`searched_range: { oldest_ts_ms, newest_ts_ms, scanned_count }`。
- 兼容：旧客户端忽略未知字段。

## CLI

`crates/bifrost-cli/src/cli.rs` Search 子命令新增：

- `--req-json <path=value>`（可重复）
- `--res-json <path=value>`（可重复）
- `--req-header <name=value>`（可重复）
- `--res-header <name=value>`（可重复）
- `--since <duration>`、`--until <duration>`
  - `30s/5m/2h/1d`：相对 `now`
  - RFC3339（如 `2026-06-17T10:00:00Z`）：绝对时间
- `--latest`：等价于 `--limit 1`，且 `max_results=1`。
- 在 `OutputFormat` 中加 `Ndjson`（保留 `Json/JsonPretty`）。

`run_simple_search`：
- 对 `json/json-pretty/ndjson` 走专门 collector：收集 `results`、`searched_range`、`time_range` 并整体输出/逐条输出。

## 远端 `bifrost remote traffic search`

同步在 `RemoteSearchArgs` 中加入 `--req-json/--res-json/--req-header/--res-header/--since/--until/--latest`；`command_search_args` 把它们映射成 `FilterCondition` 与 `time_range`。

## 兼容性 / 风险

- 既有 `FilterCondition` 分支保持不变；只在不识别 field 时走新分支。
- 旧服务端遇到带 `time_range` 的请求时（`time_range` 是 Option），`serde` 会忽略未知字段 / 默认 None。
- `searched_range` 是新字段，旧 CLI 解析时忽略。
- 性能：body cache 限制为单次 `search_internal` 调用范围；JSON 解析失败的 record 一次性标记 NonJson 不重复解析。

## 测试

- 单测：`json_path.rs` 8 个、`engine.rs` 4 个（req body 命中 / res body 命中 / ts 区间 / 非 JSON body 不匹配）、CLI duration 解析 4 个、`command_search_args` 映射 1 个。
- human_tests/search-jsonpath.md：6 个用例（since/until/latest/req-json/res-header/混合）。
- 全量：`cargo test -p bifrost-admin -p bifrost-cli -p bifrost-command`。
