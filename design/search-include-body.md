# Search Include Body / Headers + Traffic Batch Get

## 背景

`ai-report` / `traffic search` 的现有交互是：先调 `bifrost search` 拿命中 id 列表，再对每个 id 串行调用 `bifrost traffic get <id> --request-body --response-body` 拉取明细。 N 条命中即 N+1 次往返，链路放大后会显著拖慢 LLM 报告类工具的端到端时延，也让 `bifrost remote traffic search` 在批量 5xx/异常巡检场景下不可用。本设计在不改 SSE / 不改 SQL schema 的前提下，让 `bifrost search` 一次性把 body 与 header 带回来，并新增 `traffic batch get`，把 N+1 直接压成 1。

## 目标

1. `SearchRequest` 新增 `include` 子对象（可选），允许调用方按需勾选要随结果一并返回的明细：
   - `request_body` / `response_body`（默认 `false`）
   - `request_headers` / `response_headers`（默认 `false`）
   - `max_body_bytes`（默认 64 KiB；上限 8 MiB；超出自动截断并打 `truncated=true`）
2. CLI 新增 `bifrost search --include req-body,res-body,req-headers,res-headers --max-body <N>`。
   - `--include` 接逗号分隔 token，支持别名 `request-body|req-body`、`response-body|res-body`、`request-headers|req-headers|headers` 等；`headers` 与 `bodies` 是同时勾选请求/响应两侧的快捷写法。
   - 未传 `--include` 时整个 `include` 块从 wire JSON 中省略，保持向后兼容。
3. HTTP 新增 `GET /_bifrost/api/traffic/batch?ids=1,2,3&include=request-body,response-body&max_body=65536`：
   - 响应是 `application/x-ndjson`，逐行流式写出每条命中（命中找不到则写一行 `{"id":"...","error":"not_found"}`），便于大批量场景下的 backpressure。
   - 单次最多 200 个 id（`BATCH_GET_MAX_IDS`），超出 400 `[traffic.batch.too_many_ids]`。
4. CLI 新增 `bifrost traffic get --ids 1,2,3 [--max-body N]`：
   - 与现有位置参数 `<ID>` 互斥（clap `conflicts_with`）。
   - 默认输出 `ndjson`（直透服务端 line-delimited 流）；`--format json|json-pretty` 时本地聚合成 `{"results":[...]}` 信封。

## 非目标

- 不在本任务做敏感字段脱敏（Authorization / Cookie / API key）。本期 search include 与 batch get 返回已捕获的原始 header/body；完整脱敏方案另开需求设计与实现。
- 不引入 streaming pagination/cursor（200 id 上限够 wave-2 用，更大批量等 P3）。
- 不改 body 落盘格式、不改 SearchEngine 已有 `body_cache`、不改 SSE search 协议。
- 不破坏既有 `bifrost traffic get <ID>` 单 id 行为与 `--request-body/--response-body` flag。

## 协议变更

### SearchRequest.include（admin 侧）

```jsonc
{
  "query": "errno",
  "include": {
    "request_body": true,
    "response_body": true,
    "request_headers": false,
    "response_headers": true,
    "max_body_bytes": 65536
  }
}
```

- 全量字段都带 `#[serde(default)]`；旧客户端不传 `include` 时 deserialize 出 `IncludeOptions::default()`（所有 false / `max_body_bytes = None`），SearchEngine 走原路径，零开销。
- `IncludeOptions::body_limit()` 统一返回 `usize`：`max_body_bytes.unwrap_or(64 * 1024).min(8 * 1024 * 1024)`。
- 服务端在 `SearchResponse.results[i]` 上新增可选字段：
  - `bodies.request.{content_type, size, truncated, data_b64}` / `bodies.response.{...}`
  - `headers.request: [[name, value], ...]` / `headers.response: [...]`
  - body 一律 base64 STANDARD 编码（即便已知是文本），避免 UTF-8 边界 / 二进制 / JSON-in-JSON 转义噩梦；解码由 CLI 侧负责。

### GET /api/traffic/batch（admin 侧）

- query 参数：
  - `ids`：逗号分隔，必填，最多 `BATCH_GET_MAX_IDS = 200`。
  - `include`：与上面同名 token；快捷别名一致。未传时仅返回 compact 摘要（与 `traffic get <ID>` 不带 body flag 等价）。
  - `max_body`：等价 `IncludeOptions.max_body_bytes`，缺省 64 KiB。
- 响应：`application/x-ndjson`，每行：
  ```json
  {"id":"42","summary":{...},"bodies":{...?},"headers":{...?}}
  {"id":"99","error":"not_found"}
  ```
- 失败码（HTTP 400）：`[traffic.batch.missing_ids]` / `[traffic.batch.too_many_ids]` / `[traffic.batch.invalid_include_token]`。

## CLI 变更

```
bifrost search --include req-body,res-body --max-body 65536 keyword
bifrost search --include headers,bodies --format json-pretty keyword
bifrost traffic get --ids 1,2,3 --max-body 65536
bifrost traffic get --ids 1,2,3 --format json-pretty   # 信封 {"results":[...]}
bifrost traffic get 42 --request-body --response-body  # 原行为不变
```

- `--ids` 与位置参数 `<ID>` clap `conflicts_with` 严格互斥，给错会直接 usage error。
- `--include` token 大小写不敏感；未识别 token 客户端侧静默忽略（服务端侧返回 400，行为不一致是有意：CLI 友好、服务端严格契约）。

## 向后兼容

- `SearchRequest.include` 整块 `#[serde(default)]` + `IncludeOptions` 全字段 default false / None。
- `TrafficGetArgs` 新增 `ids: Vec<String>` / `max_body: Option<usize>` 等字段全部 `#[serde(default)]` + `#[derive(Default)]`，远端旧 admin 收到新 CLI 请求时 deserialize 不报错，且空 `ids` 走老路径。
- `SearchResponse.results[i].bodies` / `.headers` 字段 `#[serde(skip_serializing_if = "Option::is_none")]`，旧客户端解析新 server 输出会原样忽略。

## 风险与后续

1. **敏感数据泄漏**：当前 batch + include 直接把 Authorization / Cookie / 业务 token 明文回传给已授权 caller。完整脱敏方案另开需求处理；落地前不要把 batch/search include 输出转发给低信任 caller 或写入可复用文档。
2. **payload 放大**：base64 比原文膨胀 ~33%。对超过 1 MiB 的响应体应在调用侧自觉收紧 `--max-body`，必要时退回 `traffic get <ID>` 单条流式拉取。
3. **ndjson 客户端兼容**：浏览器 fetch / 旧版 curl 不直接支持 line-delimited；为此 CLI 提供 `--format json|json-pretty` 信封降级，但 ai-report 等脚本工具应优先消费 ndjson 避免内存峰值。
4. **200 id 上限**：足以覆盖一次报告窗口的命中量；若未来 LLM 报告窗口扩到 >200 命中，应在 P3 引入 cursor + chunk。
