# Transparent Response Headers

## 背景

Bifrost 走代理时会把 upstream 的响应转发给客户端；即使当前请求没有命中任何规则，也需要把 upstream 的响应 header 尽量透传下去，避免出现"没打规则但 header 被改了"的假象。

用户在生产上抓到过一个 case：一条没有命中任何规则的 HTTP 请求（`has_rule_hit=false`），Traffic 详情里 `original_response_headers` 有 `content-length: 168`，`response_headers` 却把 `content-length` 丢了。根因是响应侧的"skip body processing"分支永远按 unknown-length stream 归一化，即使 upstream 的长度是已知且 body 原样转发也会被删掉 `Content-Length`。

本方案在 `handler.rs` 与 `tunnel/mod.rs` 增加一个小的 streaming mode 选择器：只要 upstream `Content-Length` 可解析、无 trailer 注入、body 未被改写，就保留原 `Content-Length` 并选 `StreamWithLength`；否则退回 `Stream`（unknown-length）。

## 用户目标验证清单

### 必须实现

- 无规则、无 response body rule、无 response script、无 DevTools bridge / badge / trailer 注入的响应，透传时保留 upstream 原 `Content-Length`。
- 若 upstream 返回 chunked（无 `Content-Length`），透传时不添加假的 `Content-Length`，仍走 unknown-length stream。
- 若添加了 trailer，选 `StreamWithTrailers`，`Transfer-Encoding: chunked` 优先，`Content-Length` 移除。
- 若 response header 规则显式删除 `Content-Length` 或值不可解析，退回 unknown-length stream，安全兜底。
- 该行为同时适用于 HTTP 路径 (`handler.rs`) 与 HTTPS tunnel 路径 (`tunnel/mod.rs`)。

### 必须不破坏

- 已有 `normalize_res_headers` 对 unknown streams、trailers、no-body statuses、buffered body 的行为不变。
- Buffered body 改写路径继续用 `buffered_res_body_mode`，`Content-Length` 由 body 长度精确重算。
- Request 侧 body_mode 逻辑保持独立，不受本次改动影响。
- 无 admin API、无 storage schema、无 WebUI 前端变更。

### 必须真实验证

- 用一个显式返回 `Content-Length: 168` 的本地 origin 起来，通过 bifrost 代理拉一次不命中任何规则，客户端和 traffic detail 里 `Content-Length` 都保留。
- 用 chunked upstream 拉一次，client 收到的仍是 chunked；`Content-Length` 未被伪造。
- 触发 trailer 注入路径，`Transfer-Encoding: chunked` 被保留，`Content-Length` 被移除。

## 产品语义

### "透传"是显式承诺，不是副作用

Bifrost 面向抓包/调试用户，当 `has_rule_hit=false` 或"skip body processing"分支被走时，用户的心智模型是"什么都没做"。任何 upstream 已知信息的丢失都属于代理层引入的偏差，等同于抓包工具说谎。

因此当 body 没有被改写、trailer 没有加、response header 也没被规则动过 `Content-Length`，透传就必须选 `StreamWithLength(len)`，而不是把已知长度扔掉退化成 `Stream`。

### 选择顺序

Streaming 响应的 body mode 选择遵循以下顺序：

1. 添加 trailer → `StreamWithTrailers`（`Transfer-Encoding: chunked`，不带 `Content-Length`）。
2. 最终响应 headers 里能解析出 `Content-Length` → `StreamWithLength(len)`（保留 `Content-Length`，去掉 `Transfer-Encoding`）。
3. 其它情况 → `Stream`（unknown length，去掉 `Content-Length`）。

这个顺序放在 `streaming_res_body_mode(content_length, has_trailers)` 里，一次计算，两条路径复用。

### Buffered vs Streaming

- Buffered path：body 落地后大小已知，走 `buffered_res_body_mode(final_body.len(), has_trailers)`，`Content-Length` 由最终 body 计算。本方案不动。
- Streaming path：body 不落地，`Content-Length` 依赖 upstream 或规则显式声明，走 `streaming_res_body_mode(content_length, has_trailers)`。本方案在这里加保留分支。

## 技术细节

### 模块

- `crates/bifrost-proxy/src/proxy/http/body_metadata.rs`
  - `pub enum BodyMode { Known(usize), Stream, StreamWithLength(usize), StreamWithTrailers }`
  - `pub(in crate::proxy::http) fn streaming_res_body_mode(content_length: Option<usize>, has_trailers: bool) -> BodyMode`
  - `pub(in crate::proxy::http) fn buffered_res_body_mode(content_length: usize, has_trailers: bool) -> BodyMode`
  - `pub(in crate::proxy::http) fn normalize_res_headers(parts: &mut Parts, mode: BodyMode, method: &Method)`
- `crates/bifrost-proxy/src/proxy/http/handler.rs`
  - HTTP 转发在 skip-body-processing 分支调用 `streaming_res_body_mode(res_content_length, !resolved_rules.trailers.is_empty())`，然后 `normalize_res_headers(&mut res_parts, res_body_mode, &method)`。
- `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs`
  - HTTPS tunnel 复用同样的 helper，避免两条路径实现漂移。

### `streaming_res_body_mode` 实现

```rust
pub(in crate::proxy::http) fn streaming_res_body_mode(
    content_length: Option<usize>,
    has_trailers: bool,
) -> BodyMode {
    if has_trailers {
        BodyMode::StreamWithTrailers
    } else if let Some(len) = content_length {
        BodyMode::StreamWithLength(len)
    } else {
        BodyMode::Stream
    }
}
```

### `normalize_res_headers` 行为

- `Known(len)` / `StreamWithLength(len)`：保留 `Content-Length: len`，去掉 `Transfer-Encoding`。
- `Stream` / `StreamWithTrailers`：去掉 `Content-Length`，设置 `Transfer-Encoding: chunked`。
- HEAD / 1xx / 204 / 304 等 no-body 状态：本函数保持既有 no-body 分支。

### 计算时机

`res_content_length` 在 response header 规则应用之后才被读取（`handler.rs:3016` 附近的 `res_parts.headers.get(CONTENT_LENGTH)`），因此：

- 用户规则显式删除 `Content-Length` → `res_content_length = None` → `Stream`。
- 值非数字或超过 `usize::MAX` → `parse::<usize>()` 失败 → `None` → `Stream`。
- 未改动 → 保留 upstream 已知长度 → `StreamWithLength`。

## Sync 边界

- 本改动只在 proxy 转发层生效，不产生任何持久化或跨机器同步数据。
- Traffic recording 的 `original_response_headers` / `response_headers` 字段结构不变；只是"skip body processing"分支保存的 final headers 更贴近 upstream。
- 远端 `bifrost remote traffic get` 输出与本地一致。

## Phase 1-4

### Phase 1: body_metadata helper

- 增加 `streaming_res_body_mode`。
- `BodyMode::StreamWithLength(usize)` 分支。
- `normalize_res_headers` 处理 `StreamWithLength`：保留 `Content-Length`、去 `Transfer-Encoding`。

### Phase 2: HTTP 路径接入

- `handler.rs` 的 streaming 分支调用新 helper。
- `res_content_length` 在 header rules 应用后计算。
- 单元测试覆盖 known length / unknown / trailers 三种。

### Phase 3: HTTPS tunnel 接入

- `tunnel/mod.rs` 复用同一个 helper。
- Cross-check body mode 与 request-side normalize 不冲突。

### Phase 4: 回归 & 文档

- E2E 覆盖 no-rule + explicit content-length 透传。
- human_tests 增加 TC-PRA-61 用例。

## 测试方案

### 单元测试 (crates/bifrost-proxy/src/proxy/http/body_metadata.rs)

- `test_streaming_res_body_mode_preserves_known_length_without_trailers`：`streaming_res_body_mode(Some(168), false) == BodyMode::StreamWithLength(168)`。
- `test_streaming_res_body_mode_returns_stream_when_length_unknown`：`streaming_res_body_mode(None, false) == BodyMode::Stream`。
- `test_streaming_res_body_mode_prefers_trailers_over_known_length`：`streaming_res_body_mode(Some(168), true) == BodyMode::StreamWithTrailers`。
- `test_normalize_res_headers_preserves_stream_content_length_when_known`：已知长度 stream 保留 `Content-Length`、去 `Transfer-Encoding`。
- `test_normalize_res_headers_removes_content_length_for_unknown_stream`：未知长度 stream 去 `Content-Length`，加 `Transfer-Encoding: chunked`。
- `test_normalize_res_headers_removes_content_length_for_trailers`：trailer 分支去 `Content-Length`。

### E2E

- `e2e-tests/tests/test_no_rule_content_length_transparency.sh`：
  - 起 local origin 带 `Content-Length: 168`。
  - 起 bifrost 使用临时 data dir + `--no-system-proxy`。
  - 发一条不命中规则的请求。
  - 断言：
    - client 收到的响应保留 `Content-Length`；
    - Traffic detail `has_rule_hit=false`；
    - `original_response_headers` 含 `content-length`；
    - `response_headers` 要么不存 modified headers，要么保留同一 `content-length`。

### human_tests

- `human_tests/proxy-rules-advanced.md`：TC-PRA-61 no-rule content-length 透传回归。

启动 bifrost 使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 与 `--no-system-proxy`。

### Validation

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-proxy body_metadata`
- `bash e2e-tests/tests/test_no_rule_content_length_transparency.sh`
- 执行 `human_tests/proxy-rules-advanced.md::TC-PRA-61`

## Review/Fix/Test 闭环

### 第 1 轮

- 复核 helper 是否正确处理 `trailers 优先 > known length > unknown` 三种顺序。
- 复核 HTTPS tunnel 与 HTTP 路径是否都接入。
- 复测单元测试 + `test_no_rule_content_length_transparency.sh`。

### 第 2 轮

- 检查 `res_content_length` 是否在 header rules 应用之后再取；若在应用前取会导致规则删除 `Content-Length` 时仍误保留。
- 检查 chunked upstream 是否被误加 `Content-Length`。
- 复测 TC-PRA-61 与相关 proxy-rules regression。

## 风险与决策

- **不改 buffered path**: buffered path 的 `Content-Length` 已经准确，本次不动，避免扩大改动范围。
- **不做长度校验**: 如果 upstream 声明 `Content-Length: 168` 但实际 body 更长/更短，Bifrost 不负责校验。这是抓包/透传语义的一部分；由 hyper 或客户端负责。
- **规则显式删除 `Content-Length`**: 优先尊重规则，退回 unknown stream，避免"规则说删但代理还保留"的语义漂移。
- **无 Admin API/前端改动**: 保持改动最小；观察一段时间后如需 UI 提示 `Content-Length` 保留状态，再单独设计。
