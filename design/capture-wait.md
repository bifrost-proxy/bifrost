# Capture Wait API & CLI 设计方案

## 背景

Bifrost 需要一条“等待并捕获下一条匹配请求”的能力。真实使用场景：

- 用户在 Web 端触发某个操作（点击、输入、跳转），想立刻在 CLI 侧拿到那条请求的完整 Traffic 记录。
- Agent / 脚本先把过滤条件下发，再引导用户去客户端做动作，一旦下一条请求命中就返回。
- 调试第三方 SDK 或未知 API 时，先启监听，再触发操作，避免和 push 长连接消费者抢流量。

现有 `/api/push` 长连接虽然能推流，但对“等一条就走”的场景太重：需要维护订阅状态、消费所有 Traffic、客户端要自己做过滤，写脚本非常笨。因此新增一条一次性 API + CLI：

- 服务端：`POST /_bifrost/api/capture/wait`。调用方提交一个简短的过滤条件（host / method / path 子串），服务端在该条件命中下一条新到达流量时立即返回该记录；若 timeout 内未命中则返回 `matched=false`。
- CLI：`bifrost capture wait`，并支持 `--open <url>` 在等待前打开浏览器 / 系统默认应用，触发用户操作。

## 用户目标验证清单

### 必须实现

- `POST /_bifrost/api/capture/wait` 接受 JSON body，字段 `host_contains` / `method` / `path_contains` / `req_json` / `timeout_ms`，返回 `{matched, record?, waited_ms, scanned_count}`。
- 命中：`matched=true`，`record` 是完整 `TrafficRecord`（通过 `store.get_by_id()` 加载），`scanned_count` 反映等待期间真正被 matcher 评估过的新流量数。
- 未命中：`matched=false`，`scanned_count` 反映等待期间被评估过的流量数，`waited_ms` 大致等于 `timeout_ms`。
- `timeout_ms` 默认 60000，最大 600000，超过或非法时 clamp / 返回错误。
- `PushManager::subscribe_once(matcher, timeout)` 支持并发调用；每个调用在 `traffic_db_store.subscribe()` 上开独立 receiver，命中或超时后 task 退出、receiver drop，无 leak。
- CLI `bifrost capture wait` 支持 `--host` / `--method` / `--path` / `--timeout` / `--open <url>` / `--format human|json` / `--port <port>`。
- CLI 命中输出：单行 JSON 或 human 摘要，包含 request line 与关键 header。
- CLI 超时退出码：`124`（与常见 shell timeout 一致）。
- CLI 未连接到服务时输出 `error: capture wait request failed (is bifrost running on port {}?): {}`。

### 必须不破坏

- `/api/push` 长连接订阅、`broadcast_traffic_events` 现有语义完全不变。
- `subscribe_once` 只在 `traffic_db_store.subscribe()` 上多开一个 receiver，broadcast 天然支持多消费者，不与 PushManager 内部的 broadcast 流抢资源。
- 不修改 `TrafficRecord` / `TrafficSummaryCompact` 结构。
- 不新增 body 脱敏 / 大 body 转储；返回的 `record` 字段直接取 `TrafficRecord`，调用方需把输出视为敏感数据。
- 现有 CLI 命令不受影响；`bifrost capture` 命名空间只新增 `wait` 子命令。
- `req_json` 字段接口保留，实现里第一版恒返回 true（占位），等 SearchEngine 的 JSONPath 过滤合入后再接。

### 必须真实验证

- 真跑一次：`bifrost capture wait --host api.example.com --method POST --path /api/widget --open <url> --timeout 2m`，浏览器被打开触发操作后，CLI 输出命中记录。
- 无匹配：等待到超时，退出码 `124`。
- 并发：两个 `subscribe_once` 用不同 matcher 同时等，各自命中不同流量。
- 服务端未跑：CLI 打印明确错误信息，非 `panic`。
- 大 body 请求命中时，返回记录中的 `body_ref` 能被后续 `bifrost traffic get` / `body-view` 打开。

## 产品语义

### 一条命令等一条请求

`bifrost capture wait` 的核心动作是“阻塞、命中一条、退出”。它不是订阅、不是持续消费；调用方拿到响应即可结束等待，无需维护长连接订阅状态。当用户在文档里读到 `bifrost capture wait --open ...` 时，产品语义就应该是“打开浏览器、等我做完动作、把那条请求打印出来”。

### `--open` 是可选的触发助手

`--open <url>` 只是一个便利入口：CLI 在开始等待前先调用系统默认打开器（macOS `open`、Linux `xdg-open`、Windows `start`）跳转到指定 URL，用户在页面上做操作触发请求。CLI 不管浏览器结果，只关心过滤条件是否命中。

### `req_json` 是 P0 占位

`req_json` 字段是 SearchEngine JSONPath 过滤的接口，一期实现里恒返回 true；不会因为该字段就减少 `host` / `method` / `path` 的匹配严格性。SearchEngine 集成后再把 `req_json` 接到真实过滤逻辑，同一接口向后兼容。

### 返回 `record` 是完整 TrafficRecord

第一版返回完整 `TrafficRecord`，让脚本一次性拿到所有需要的字段（method / url / status / headers / body_ref / duration / matched_rules 等）。调用方需要把该输出视为敏感数据，尤其在把 CLI 输出粘贴到聊天窗时。

## 技术细节

### 数据流

```
proxy 真实流量
   │ (TrafficStoreEvent::Inserted)
   ▼
traffic_db_store.subscribe()  ─── broadcast::Receiver<TrafficStoreEvent>
   │
   ▼
PushManager::subscribe_once(matcher, timeout)
   │  内部 spawn 一个独立 tokio task：
   │   - 持有 receiver
   │   - 收到 Inserted/Updated → 把 record 转成 TrafficSummaryCompact，先跑 matcher，
   │     命中则 send 到 oneshot，并 break。
   │   - select! 上挂 tokio::time::timeout
   │
   ▼ (Some(record) 或 None)
HTTP handler 把命中 record 通过 store.get_by_id() 拿到完整 TrafficRecord，
组装 JSON 响应。
```

不与 PushManager 内部的 `broadcast_traffic_events` 流抢资源：`subscribe_once` 在 `traffic_db_store.subscribe()` 上开一个独立 receiver，broadcast channel 天然支持多消费者；匹配器在 spawn 的 task 里跑，命中或超时后 task 退出，receiver drop，无 leak。

### API 契约

`POST /_bifrost/api/capture/wait`

请求 body：

```json
{
  "host_contains": "api.example.com",
  "method": "POST",
  "path_contains": "/api/widget",
  "req_json": [{"path": "$.chart_name", "value": "Commit基本信息"}],
  "timeout_ms": 120000
}
```

字段说明：

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `host_contains` | string | 否 | host 子串匹配，大小写不敏感 |
| `method` | string | 否 | 精确大写匹配，如 `POST` |
| `path_contains` | string | 否 | path 子串匹配 |
| `req_json` | array | 否 | P0 阶段只做接口占位，恒返回 true；SearchEngine 合入后接 |
| `timeout_ms` | u64 | 否 | 等待超时，默认 60000，最大 600000 |

响应（始终 HTTP 200）：

命中：

```json
{ "matched": true, "record": { "…完整 TrafficRecord 字段…": null }, "waited_ms": 3210, "scanned_count": 5 }
```

超时：

```json
{ "matched": false, "scanned_count": 12, "waited_ms": 60000 }
```

`scanned_count` 反映等待期间真正被 matcher 评估过的新流量数（与全量流量总数不同）。

### PushManager 增量

只新增一个对外方法，不改任何现有行为：

```rust
impl PushManager {
    /// Wait for the next traffic record whose `TrafficSummaryCompact`
    /// passes `matcher`. Returns `CaptureWaitOutcome::matched = None` if no
    /// record arrives within `timeout`. Does not interact with the existing
    /// push-broadcast loop. Multiple concurrent `subscribe_once` calls are
    /// safe: each spawns its own receiver on the traffic_db_store broadcast.
    pub async fn subscribe_once<F>(&self, matcher: F, timeout: Duration) -> CaptureWaitOutcome
    where
        F: Fn(&TrafficSummaryCompact) -> bool + Send + Sync + 'static,
    { … }
}

pub struct CaptureWaitOutcome {
    pub matched: Option<TrafficSummaryCompact>,
    pub scanned: usize,
}
```

实现要点（`crates/bifrost-admin/src/push.rs`）：

- 若当前 `PushManager` 未挂载 traffic store，`matched=None`、`scanned=0`，立即返回。
- 使用 `AtomicUsize` 累计 `scanned`。
- Task 在 `select!` 中挂 `tokio::time::timeout(timeout)`；命中或超时都退出。
- 只订阅 `TrafficStoreEvent::Inserted`（第一版）；后续如需要 `Updated`，语义扩展前先补测试。

### CLI 结构

`crates/bifrost-cli/src/commands/capture.rs`：

```rust
pub struct CaptureWaitOptions {
    pub host: Option<String>,
    pub method: Option<String>,
    pub path: Option<String>,
    pub timeout: Duration,
    pub open: Option<String>,
    pub format: CaptureOutputFormat,
    pub port: u16,
}

pub enum CaptureOutputFormat { Human, Json }
```

`run_capture_wait`：

1. 若 `open` 非空 → `try_open_url` 调用 `open` / `xdg-open` / `cmd /c start`。
2. 组装 `CaptureWaitApiRequest` 通过 `bifrost_core::direct_ureq_agent_builder()` 发 POST（避免走系统代理）。
3. 解析 `CaptureWaitApiResponse`，`matched=true` 打印记录，返回 0；`matched=false` 返回 `EXIT_TIMEOUT = 124`。
4. 网络失败 → 打印 `error: capture wait request failed (is bifrost running on port {}?): {}`，返回 2。

`parse_duration`：支持 `30s` / `2m` / `1h` / `500ms` / 裸数字（秒）。

## CLI 交互

```
bifrost capture wait --host api.example.com --method POST --path /api/widget --timeout 2m
bifrost capture wait --host example.com --open https://example.com/dashboard --format json
bifrost capture wait --path /api/login --format human
bifrost capture wait --path /api/never --timeout 10s        # 超时退 124
bifrost capture wait --path /api/x --port 19900             # 指向自定义端口
```

human 输出示例：

```
[capture] matched in 3210ms (scanned 5 records)
POST https://api.example.com/api/widget?token=…
  request headers: content-type=application/json, cookie=…
  request body: 1.2 KiB (ref: rb-2b8a…)
  status: 200 OK
  response headers: content-type=application/json
  response body: 812 B (ref: sb-1f2c…)
  matched rules: my-widget-rule
  record id: 7f2b4a…
```

JSON 输出：直接把 `record` 打印为一行 JSON，便于 `jq` / `bifrost traffic get $(jq -r .record.id ...)` 二次处理。

## Web / Admin API

- 新增 `POST /_bifrost/api/capture/wait`（`crates/bifrost-admin/src/handlers/capture.rs`）：`POST` only，其它方法返回 `method_not_allowed()`。
- OpenAPI（`crates/bifrost-admin/src/openapi.rs`）新增该端点，字段与上表一致。
- Web UI 第一版不新增独立入口；未来可以在 Traffic 面板加“Wait for next request…”便捷按钮，产品语义与 CLI 一致。

## Sync 边界

- `subscribe_once` 是本地流量捕获，不参与跨设备 sync。
- 返回的 `TrafficRecord` 走本地 traffic store，不主动 sync 到 remote；如果需要在远端跑 `capture wait`，通过 `bifrost remote traffic` / `bifrost remote exec bifrost capture wait ...` 在远端执行。
- `--open <url>` 只在本地打开器执行，不通过 remote 触发。

## 实现切分

### Phase 1：PushManager::subscribe_once

- 在 `push.rs` 新增 `subscribe_once` 与 `CaptureWaitOutcome`。
- 覆盖：`subscribe_once_returns_matching_record` / `subscribe_once_times_out_when_no_match` / `subscribe_once_without_traffic_store_returns_none_immediately`。
- 保证并发调用互不干扰。

### Phase 2：Admin API handler

- `handlers/capture.rs`：POST only、请求体解析、`timeout_ms` clamp、matcher 构造（`host` lowercase / `method` uppercase / `path` 子串 / `req_json` 占位）。
- 命中后 `store.get_by_id()` 拿完整 record，组装 200 响应。
- OpenAPI 描述。

### Phase 3：CLI

- `crates/bifrost-cli/src/commands/capture.rs`：`CaptureWaitOptions` / `run_capture_wait` / `parse_duration` / `try_open_url` / `CaptureOutputFormat`。
- `EXIT_TIMEOUT=124`；网络失败输出可读错误。
- `--help` 输出 CLI 语义。

### Phase 4：文档与 human_tests

- `design/capture-wait.md`（本文）保持权威文档。
- `human_tests/capture-wait.md` 新增真实执行用例。
- `human_tests/readme.md` 索引更新。

## 测试方案

### 单元测试

- `push::subscribe_once_returns_matching_record`：并发插入 3 条 Traffic，其中第 2 条命中，`matched` 返回该条记录，`scanned >= 2`。
- `push::subscribe_once_times_out_when_no_match`：matcher 恒 false，200ms 超时后 `matched=None`，`scanned >= 1`。
- `push::subscribe_once_without_traffic_store_returns_none_immediately`：未挂 traffic store 时立即返回 `matched=None`、`scanned=0`。
- `handlers::capture_wait_rejects_non_post`：`GET /_bifrost/api/capture/wait` → 405。
- `handlers::capture_wait_matches_host_and_method`：POST + `host_contains` + `method`，命中后返回 `matched=true` 与 `record`。
- `handlers::capture_wait_timeout_returns_matched_false`：`host_contains` 永远不匹配，等 150ms 后返回 `matched=false`。
- `handlers::capture_wait_default_timeout`：不传 `timeout_ms` → 使用默认 60000；超过 600000 → clamp 到 600000。
- `cli::parse_duration_supports_common_units`：`500ms`、`30s`、`2m`、`1h`、裸数字都能解析。
- `cli::run_capture_wait_returns_124_on_timeout`：mock server 返回 `matched=false` 时 CLI 退出码 124。
- `cli::run_capture_wait_prints_record_on_match`：mock server 返回 `matched=true` 时 CLI 打印 record，退出码 0。

### E2E 测试

- `e2e-tests/tests/test_capture_wait_match.sh`：启动 bifrost + mock upstream，`bifrost capture wait --host mock --path /widget --timeout 5s &`，随后 `curl` 触发；CLI 命中打印记录并退出。
- `e2e-tests/tests/test_capture_wait_timeout.sh`：`bifrost capture wait --host never --timeout 1s`，无流量触发，退出码 124。
- `e2e-tests/tests/test_capture_wait_open.sh`：`--open <mock URL>`，`try_open_url` 使用测试替身，验证触发命令被执行；随后 mock 流量命中。
- `e2e-tests/tests/test_capture_wait_no_server.sh`：不启动 bifrost，CLI 打印 `error: capture wait request failed (is bifrost running on port …)`，退出码 2。

### 真实场景测试 human_tests

新增 `human_tests/capture-wait.md`：

- `TC-CAP-01`：本地打开一个可控页面，用 `--open` + `--host` + `--path` 捕获真实提交请求，验证输出 JSON 包含正确 record id。
- `TC-CAP-02`：无匹配等到超时，验证退出码 124。
- `TC-CAP-03`：并发两个 `capture wait` 用不同 matcher 同时等，各自命中不同流量。
- `TC-CAP-04`：使用 `bifrost traffic get <record-id>` 二次读取命中的 record，验证 body_ref 可读。
- 服务启动使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 和 `--no-system-proxy`。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo test -p bifrost-admin push subscribe_once`
- `cargo test -p bifrost-admin handlers::capture`
- `cargo test -p bifrost-cli capture`
- `bash e2e-tests/tests/test_capture_wait_match.sh`
- `bash e2e-tests/tests/test_capture_wait_timeout.sh`
- `cargo test --workspace --all-features`
- `rust-project-validate`

本机 no-local-coverage 约定时不跑 `make coverage`；交付时说明本地豁免。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：一次性命中、超时可控、CLI `--open` 便利、并发安全、`req_json` 占位、退出码 124、错误文案。
- 复核 diff：`push.rs::subscribe_once` / `handlers/capture.rs` / `commands/capture.rs` / OpenAPI / 单元测试与 E2E。
- 重点 review：并发 receiver 是否 leak；`scanned_count` 是否只计入被 matcher 评估的流量；网络失败错误文案是否稳定；`timeout_ms` clamp 边界。
- 复测：focused 单元测试 + 4 条 E2E + human_tests。

### 第 2 轮

- 复查第 1 轮修复；再次 `git status --short` / `git diff`。
- 重点 review：`--open` 在 macOS / Linux / Windows 三平台 dispatch 是否一致；`try_open_url` 失败是否阻塞等待（当前只 warning，不阻塞）；CLI 输出 JSON 是否是单行；`bifrost capture wait` help 是否完整。
- 复测：失败路径重跑、`cargo test --workspace --all-features`、`rust-project-validate`。

## 风险与决策点

- **返回完整 `TrafficRecord`**：便利但敏感。第一版不做脱敏；文档中提示“把 CLI 输出视为敏感数据”。若后续需要脱敏，通过新增 `--strip-body` / `--strip-headers` CLI 开关和对应 API 字段实现，不改默认行为。
- **`req_json` 占位**：一期实现里恒返回 true，避免阻塞 SearchEngine 集成节奏；等 SearchEngine 合入后再把它接到真实过滤逻辑。文档明确“P0 占位”，避免调用方以为它已经生效。
- **不订阅 `Updated` 事件**：第一版只订阅 `Inserted`。SSE / WebSocket 会有 `Updated` 事件，但典型的“等下一条 request”场景是 `Inserted`；后续如果需要 `Updated`，扩展 API 前先补测试。
- **`--open` 不阻塞**：即使 `open` / `xdg-open` 命令失败，也只打印 warning，继续等待；用户可以手工触发请求。这样保证 CLI 语义可预测。
- **timeout 上限 600s**：足够覆盖“打开浏览器、登录、触发操作”场景；再长的 timeout 建议脚本自己 loop，避免长时间挂住连接。
- **端口默认值**：CLI `--port` 默认 9900，与 Bifrost admin 默认端口一致；用户在非默认端口时必须显式传入。
