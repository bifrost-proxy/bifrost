# Search Include Body / Headers + Traffic Batch Get

## 背景

`ai-report`、LLM 报告类工具与 `bifrost remote traffic search` 都遵循相同的调用模式：先调 `bifrost search` 拿到命中列表，再逐条 `bifrost traffic get <id> --request-body --response-body` 拉取明细。命中 N 条即产生 N+1 次远端往返，在远程巡检、5xx 批量分析、prompt/response 快查场景下时延放大严重。

本设计在不改 SSE 协议、不改 SQL schema、不改 body 落盘格式的前提下：

1. 让 `bifrost search` 一次性把 body / headers 按需带回来。
2. 新增 `GET /api/traffic/batch`，`bifrost traffic get --ids` 一次拉多条，输出 `application/x-ndjson`。

把 N+1 压缩成 1 次调用，同时保持旧客户端 wire-compat。

## 用户目标验证清单

### 必须实现

- `SearchRequest` 新增可选 `include` 子对象，字段 `request_body` / `response_body` / `request_headers` / `response_headers` / `max_body_bytes`。
- CLI `bifrost search --include req-body,res-body,req-headers,res-headers --max-body <N>`；支持别名（`req-body|request-body`、`headers` = 请求 + 响应两侧、`bodies` = 请求 + 响应两侧）。
- `SearchResponse.results[i]` 新增可选 `bodies` / `headers` 字段，body 一律 base64 STANDARD 编码。
- 新增 `GET /api/traffic/batch?ids=1,2,3&include=...&max_body=...`：响应 `application/x-ndjson`，单次最多 200 个 id。
- CLI `bifrost traffic get --ids 1,2,3 [--max-body N]`：默认输出 ndjson，`--format json|json-pretty` 时聚合成 `{"results":[...]}`。
- `--ids` 与位置参数 `<ID>` clap 严格互斥。

### 必须不破坏

- 旧客户端不传 `include` 时 `SearchInclude::default()`（全 false，`max_body_bytes = None`），SearchEngine 走原路径，无额外解析。
- `bifrost traffic get <ID> --request-body --response-body` 单 id 行为不变。
- SSE search 事件协议、SQL schema、`BodyStore` 落盘格式都不动。
- 200 id 上限之内的批量对旧 admin 无影响；旧 admin 收到 `include` 未识别字段按 `#[serde(default)]` 忽略。

### 必须真实验证

- LLM 报告类调用：一次 `search --include req-body,res-body` 得到全部命中的正文，无需二次调用。
- `bifrost traffic get --ids 1,2,3` 与串行 3 次 `traffic get` 结果 union 一致。
- `--ids` 与 `<ID>` 互斥错误信息由 clap 直接给出。
- ndjson 逐行流式，超大 body 也不会撑爆客户端内存。

## 产品语义

### body 一律 base64 STANDARD

即便已知是文本，服务端也不做 UTF-8 判断，避免二进制 / JSON-in-JSON / MIME 边界问题。CLI 收到后按需 decode。

### `max_body_bytes` 兜底

- 默认 64 KiB (`SearchInclude::DEFAULT_MAX_BODY_BYTES = 64 * 1024`)。
- 上限 8 MiB（`min(8 * 1024 * 1024)`）。
- 超出的 body 截断并在返回体上打 `truncated=true`。

### `--include` 服务端严格、客户端宽松

- 服务端 `GET /api/traffic/batch?include=xxx` 遇到未识别 token 返回 `[traffic.batch.invalid_include_token]`（HTTP 400）。
- CLI 遇到未识别 token 静默忽略（易用性优先），并在 verbose 模式下 warn。

## 技术细节

### `SearchInclude`

`crates/bifrost-admin/src/search/types.rs:29`：

```rust
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SearchInclude {
    #[serde(default)]
    pub request_body: bool,
    #[serde(default)]
    pub response_body: bool,
    #[serde(default)]
    pub request_headers: bool,
    #[serde(default)]
    pub response_headers: bool,
    #[serde(default)]
    pub max_body_bytes: Option<usize>,
}

impl SearchInclude {
    pub const DEFAULT_MAX_BODY_BYTES: usize = 64 * 1024;
    pub fn any(&self) -> bool { ... }
    pub fn body_limit(&self) -> usize {
        self.max_body_bytes
            .unwrap_or(Self::DEFAULT_MAX_BODY_BYTES)
            .min(8 * 1024 * 1024)
    }
}
```

`SearchRequest.include: SearchInclude` 带 `#[serde(default)]`，旧客户端不传时 deserialize 出 default。

### `SearchResultItem`

`crates/bifrost-admin/src/search/types.rs:200`：

```rust
pub struct SearchResultItem {
    // 老字段保持不变。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bodies: Option<BodiesPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<HeadersPayload>,
}
```

`BodiesPayload { request: Option<BodyPart>, response: Option<BodyPart> }`；`BodyPart { content_type, size, truncated, data_b64 }`。旧客户端解析新 server 时按 `skip_serializing_if` 直接跳过未知字段。

### `/api/traffic/batch`

`crates/bifrost-admin/src/handlers/traffic.rs:1158` 附近：

```rust
const BATCH_GET_MAX_IDS: usize = 200;
const BATCH_GET_DEFAULT_MAX_BODY_BYTES: usize = 64 * 1024;
```

Query 参数：
- `ids`（必填，逗号分隔，去空、去重；超过 200 → 400）。
- `include`（可选）：与 `SearchInclude` 同名 token，别名同上。
- `max_body`（可选，默认 64 KiB）。

响应：`application/x-ndjson`，每行：
```json
{"id":"42","summary":{...},"bodies":{...?},"headers":{...?}}
{"id":"99","error":"not_found"}
```

失败码：
- `[traffic.batch.missing_ids]`
- `[traffic.batch.too_many_ids]`
- `[traffic.batch.invalid_include_token]`

### CLI

`crates/bifrost-cli/src/commands/traffic.rs`：

```
bifrost traffic get 42 --request-body --response-body        # 老路径
bifrost traffic get --ids 1,2,3 --max-body 65536             # ndjson
bifrost traffic get --ids 1,2,3 --format json-pretty         # 聚合 {"results":[...]}
```

- `TrafficGetOptions` 新增 `ids: Vec<String>`、`max_body: Option<usize>`，clap `conflicts_with = "id"`。
- `run_traffic_batch_get` 走 `/api/traffic/batch`，直接 stream 服务端 ndjson。
- `--format json|json-pretty|ndjson`：`json/json-pretty` 客户端聚合。

`crates/bifrost-cli/src/commands/search.rs`：

- `--include` 逗号分隔 token 解析成 `SearchInclude` 四个 bool + `max_body_bytes`。
- `--max-body <N>` 覆盖 `max_body_bytes`。
- token 别名（大小写不敏感）：
  - `request-body|request_body|req-body|reqbody`
  - `response-body|response_body|res-body|resbody`
  - `request-headers|req-headers|req-header`
  - `response-headers|res-headers|res-header`
  - `headers` = 请求 + 响应 headers
  - `bodies` = 请求 + 响应 bodies

### `RemoteSearchArgs`

`crates/bifrost-cli/src/cli/remote.rs`、`crates/bifrost-cli/src/commands/remote.rs`：`bifrost remote traffic search` / `bifrost remote traffic get` 同步 `--include` / `--max-body` / `--ids`，`command_search_args` 把它们映射进 `SearchRequest.include`。

## CLI / Web / Admin API 快照

| 层 | 入口 | 能力 |
|---|---|---|
| CLI | `bifrost search --include ... --max-body ...` | 一次带回 body/headers |
| CLI | `bifrost traffic get --ids ...` | 批量拉明细 |
| CLI | `bifrost remote traffic search/get` | 远端同能力 |
| Admin API | `POST /api/traffic/search` + `include` | 单次带 body/headers 命中 |
| Admin API | `GET /api/traffic/batch` | ndjson 批量 |
| Web | Traffic Search 面板 | 复用现有搜索 UI，body/header 展开由 `include` 驱动 |

## Sync 边界

Traffic 数据存本地 SQLite，`traffic search/batch` 不与远端 sync 交互。远端调用走 `bifrost remote invoke`（TLS + relay），本设计不改远端加密语义。

## Phase 拆分

- **Phase 1**：`SearchInclude` 类型 + `SearchResponse.results[i].bodies/headers` 字段 + Search Engine hydration。
- **Phase 2**：`/api/traffic/batch` handler + `BATCH_GET_MAX_IDS` + ndjson 输出。
- **Phase 3**：CLI `--include` / `--max-body` / `--ids` / `--format ndjson` + 别名解析。
- **Phase 4**：`bifrost remote traffic search/get` 映射 + ai-report 侧改造使用 batch 接口。

## 测试方案

### 单元测试

- `crates/bifrost-admin/src/search/types.rs`：
  - `search_request_deserializes_without_include_field`
  - `search_include_deserializes_partial_fields`
- `crates/bifrost-admin/src/handlers/traffic.rs::batch_query_tests`：
  - `parse_batch_traffic_query` 基本、`include` token、`max_body` override
  - `missing_ids` / `too_many_ids` / `invalid_include_token`
  - `parse_batch_traffic_query` 去空/去重
- `crates/bifrost-cli/src/commands/search.rs`：`--include` token 解析、别名映射
- `crates/bifrost-cli/src/commands/traffic.rs`：`--ids` 与 `<ID>` 互斥 clap 错误

### E2E 测试

- `e2e-tests/tests/test_search_traffic_cli_isomorphic_e2e.sh`：
  - `search --include req-body,res-body` 返回 base64 body。
  - `traffic get --ids 1,2,3 --format json` 聚合结果与 3 次 `traffic get` union 相等。
  - 超过 200 id 返回 `[traffic.batch.too_many_ids]`。
- `bifrost search --include bodies --format json-pretty <keyword>` 手工验证 base64 解码。

### 真实场景测试 human_tests

`human_tests/search-include-body.md`：

- TC-SIB-01：`bifrost search --include req-body,res-body --max-body 4096 errno` 命中并返回截断 body。
- TC-SIB-02：`bifrost traffic get --ids 1,2,3` ndjson 逐行输出。
- TC-SIB-03：`bifrost traffic get --ids 1,2,3 --format json-pretty` 聚合信封。
- TC-SIB-04：`--ids` 与 `<ID>` 互斥被 clap 拒绝。
- TC-SIB-05：`include=mystery-token` 服务端 400，CLI 侧忽略不识别 token。
- TC-SIB-06：超过 200 id `[traffic.batch.too_many_ids]`。

### 覆盖率与项目校验

- `cargo test -p bifrost-admin search::types include_serde`
- `cargo test -p bifrost-admin handlers::traffic batch_query_tests`
- `cargo test -p bifrost-cli commands::search`
- `cargo fmt --all -- --check` / clippy workspace 全绿
- 本地不跑 coverage，交给远端 CI

## Review / Fix / Test 闭环

### 第 1 轮

- 复核 wire compat：`SearchInclude` `#[serde(default)]`、`SearchResultItem.bodies/headers` `skip_serializing_if`、旧 CLI 与旧 admin 双向兼容。
- 复核 `BATCH_GET_MAX_IDS = 200`、`BATCH_GET_DEFAULT_MAX_BODY_BYTES = 64 KiB`、`SearchInclude::body_limit` 上限 8 MiB。
- 复核 ndjson 输出顺序与 `ids` 请求顺序一致；缺失 id 写 `error=not_found`。

### 第 2 轮

- 复核 `--include` token 别名、大小写不敏感、CLI 侧忽略未识别 token。
- 复核 remote invoke 端 `command_search_args` 映射 `include/max_body/ids`。
- 复跑 E2E 与 CLI 手工验证脚本。

## 风险与决策

- **敏感数据泄漏**：batch + include 会把 Authorization / Cookie / API token 明文返回给已授权 caller。第一版**不做**脱敏，后续单独设计脱敏方案；落地前不要把 batch/include 输出转发给低信任 caller 或写入可复用文档。
- **payload 放大**：base64 比原文膨胀 ~33%。>1 MiB 响应体应主动收紧 `--max-body`，或退回单条流式 `traffic get <ID>`。
- **ndjson 客户端兼容**：老 curl / 浏览器 fetch 不直接支持 line-delimited；CLI 提供 `--format json|json-pretty` 降级信封，但脚本工具优先消费 ndjson 避免内存峰值。
- **200 id 上限**：当前 wave-2 场景够用；若未来 LLM 报告窗口超过 200 命中，P3 引入 cursor + chunk。
