# Replay Local Forwarding

## 背景

Bifrost 的普通代理链路在 `bifrost.local http://localhost:5173/` 这类本地开发转发规则下会自动：

- 用规则改写后的 URL 重建传输层请求：新的 `Host`、新的 `Content-Length` 与 hop-by-hop 头。
- 对 HTTP/2 pseudo header 做过滤。

Replay 的 unified HTTP 执行路径 (`crates/bifrost-admin/src/handlers/replay.rs`) 长期使用独立的 `reqwest` 请求构造逻辑，规则改写 URL 后仍逐个转发原始请求头，旧的 `Host: bifrost.local`、`Content-Length`、`Connection` 等传输级头会继续进入本地上游，导致：

- 本地 dev server / proxy 链路收到不符合当前 body 长度的 `Content-Length`，返回 502 或直接断开。
- HTTP/1.1 客户端向 HTTP/2 上游发起时，pseudo header 泄漏引发 protocol error。
- 原 Host 出现在本地 Vite 服务后被视为跨域请求，触发额外错误。

同时线上使用的 nextoncall PPE 规则集包含更高优先级的 API 透传规则：

```text
https://bifrost.local/api/nextagent/ passthrough://
bifrost.local http://localhost:5173/
```

Replay 的规则应用层原本只把 `passthrough://` 记录为“匹配到”，但并没有阻断后续域名级 `http://localhost:5173/` 转发，导致本该透传的 `/api/nextagent/` 被错误地重写到本地 Vite，形成 502。

此外，Replay 从规则存储读取规则时直接使用普通 `parse_rules`，没有解析规则文件里的 markdown inline values。`reqHeaders://{ppe2}` 这种写法在 replay 路径里会保留成未展开占位符，无法真实注入 `x-tt-env` / `x-use-ppe`。

## 用户目标验证清单

### 必须实现

- Replay 转发到本地上游时，`Host` 由 HTTP client 根据最终 URL 生成，不再泄漏原远端 `Host`。
- 传输级 hop-by-hop 头（`Content-Length`、`Transfer-Encoding`、`Connection`、`Proxy-Connection`、`Keep-Alive`、`TE`、`Trailer`、`Upgrade`）与 HTTP/2 pseudo header（`:authority`、`:method`、`:path`、`:scheme` 等）不进入本地上游。
- `Passthrough://` 命中后清空已记录的转发目标与 host 覆盖，阻断后续所有转发类规则。
- Replay 规则解析统一走 `RuleParser::parse_rules_with_inline_values`，把规则文件的 markdown inline values 合入 resolver value store。
- `reqHeaders://{expr}` 展开出的多行文本按 `\n` 拆分成多个 header 一起注入。

### 必须不破坏

- Replay 应用语义头（`Authorization`、`Content-Type`、业务自定义头）继续原样转发。
- WebSocket replay 保持既有独立握手过滤逻辑，本次不改。
- 目标 URL 与原请求 authority 相同（例如未命中转发规则）时，用户手动设置的 `Host` header 仍生效。
- 普通代理链路 request/response 头行为不变。

### 必须真实验证

- 单元测试覆盖：授权 host 切换、host 保持、hop-by-hop 过滤、passthrough 阻断、多行 header 解析。
- E2E 通过 mock HTTP echo server 断言：本地上游收到 `/api/nextagent/v1/sessions`；未泄漏 `Host: bifrost.local` / `Connection`。

## 产品语义

Replay 必须与普通代理链路共享以下语义：

1. **HTTP forwarding 头重建**：目标 URL authority 变化时，Host / Content-Length / hop-by-hop 头由 HTTP client 根据最终 body 与 URL 重新生成。
2. **Passthrough 短路**：`passthrough://` 命中后，后续所有 forward / host / xhost 类规则失效，请求以“透传到原目标”执行。
3. **规则文件 inline values 参与解析**：Replay 与代理共用 `parse_rules_with_inline_values`，`{ppe2}` 等占位符可展开成实际值。
4. **多行 header 拆分**：`reqHeaders` 的 resolved value 支持 `\n` 分隔多条 header，全部注入请求。

## 技术细节

### 头过滤：`should_skip_http_forward_header`

`crates/bifrost-admin/src/handlers/replay.rs` 提供辅助函数（line 3524 附近）：

```rust
fn should_skip_http_forward_header(name: &str, host_changed: bool) -> bool {
    // 1. 空 / 非法头名跳过
    // 2. HTTP/2 pseudo header (以 ':' 开头) 跳过
    // 3. hop-by-hop 头集合命中跳过
    // 4. host_changed=true 时跳过 "Host"
}
```

hop-by-hop 头集合：

```rust
const HOP_BY_HOP: &[&str] = &[
    "Content-Length",
    "Transfer-Encoding",
    "Connection",
    "Proxy-Connection",
    "Keep-Alive",
    "TE",
    "Trailer",
    "Upgrade",
];
```

比较大小写不敏感。

### unified replay 执行流

在构造 outbound `reqwest::Request` 时：

1. 收集应用规则 headers（`reqHeaders` 展开、用户自定义 header 覆盖）。
2. 逐个复制原始 request headers；对每个 header 调用 `should_skip_http_forward_header(name, host_changed)`，命中则丢弃。
3. `host_changed = original_url.authority() != rewritten_url.authority()`。
4. `reqwest` 根据最终 URL 与 body 自动写入 `Host` 与 `Content-Length`。

### `AppliedRules` 与 passthrough

`crates/bifrost-admin/src/request_rules.rs`：

```rust
Rule::Passthrough(_) if !applied.forwarding_passthrough => {
    applied.forwarding_passthrough = true;
    applied.forward_url = None;
    applied.forward_source_path = None;
    applied.forward_target_path_exact = false;
    applied.host = None;
}
```

后续 forward / host 类规则处理时用 `!applied.forwarding_passthrough` 保护。

### inline values 参与解析

- Replay 自定义规则文本和存储规则加载统一改成 `RuleParser::parse_rules_with_inline_values`。
- inline values 合入 resolver value store，供 `resBody://(name)` / `reqHeaders://{name}` 等模板展开。

### 多行 header

`parse_header_values` 允许一个 `reqHeaders` 值展开成多行；每行按 `Name: Value` 解析，全部加入请求 header 集合。空行忽略，`#` 起头行按注释忽略。

## CLI / Web / Admin API

- Admin API 入口：`POST /_bifrost/api/replay/execute`、`.../execute/sse`、Replay WebSocket。
- Web UI：Replay 页面直接受益；预览规则匹配日志会显示 `passthrough` 命中而非继续 forward。
- CLI：无新增。

## Sync 边界

- 规则数据来源与 Sync 行为不变；本次修复只在 Replay 执行侧改写头与解析规则。

## 实现切分

### Phase 1：头过滤

- `should_skip_http_forward_header` 完备并单测。
- unified replay 构造函数接入。

### Phase 2：passthrough 短路

- `AppliedRules` 增加 `forwarding_passthrough`。
- 所有 forward / host 分支加保护。

### Phase 3：inline values + 多行 header

- 切换到 `parse_rules_with_inline_values`。
- 合入 resolver value store。
- 多行 header 拆分与注入。

### Phase 4：E2E + human_tests

- 新增 `test_forward_localhost_api_rule` 到 `test_replay_rules.sh`。
- 更新 `human_tests/webui-replay.md::TC-WRP-24`。

## 测试方案

### 单元测试

- `crates/bifrost-admin/src/handlers/replay.rs`
  - `replay_forward_skips_stale_host_when_rule_changes_target`（line 157）
  - `replay_forward_keeps_custom_host_when_authority_is_unchanged`（line 166）
  - `replay_forward_skips_hop_by_hop_headers_and_pseudo_headers`（line 175）
  - `should_skip_http_forward_header_skips_empty_and_pseudo_headers`（line 3524）
- `crates/bifrost-admin/src/request_rules.rs`
  - `test_passthrough_blocks_later_forward_rule_from_resolved_rules`（line 1316）
  - `test_parse_header_values_multiline`（line 1421）

### E2E 测试

- `e2e-tests/tests/test_replay_rules.sh`
  - `test_forward_localhost_api_rule`（line 1002，dispatch at line 1140）
    - 起本地 mock HTTP echo。
    - 规则：`bifrost.local http://127.0.0.1:<mock>/`。
    - Replay 请求 `https://bifrost.local/api/nextagent/v1/sessions`。
    - 断言上游收到 `/api/nextagent/v1/sessions`。
    - 断言旧 `Host: bifrost.local` 未泄漏。
    - 断言 `Connection` hop-by-hop 头未泄漏。
    - 组合规则场景：`https://bifrost.local/api/nextagent/ passthrough://` + `bifrost.local http://127.0.0.1:<mock>/`，断言 passthrough 命中不落到 mock。

### human_tests

- `human_tests/webui-replay.md::TC-WRP-24`（line 406）“Replay HTTPS API 请求经 localhost 规则转发/透传不返回 502”。
  - 附完整 nextoncall PPE 规则：API passthrough 命中后不再被域名级 localhost 规则覆盖，且 PPE headers（`x-tt-env`, `x-use-ppe`）真实注入。
- `human_tests/readme.md` Replay UI 一行同步描述“localhost 转发与 passthrough 优先级回归”（line 82）。

## Review / Fix / Test 闭环

### 第 1 轮

- 复核目标：Host 换新 / hop-by-hop 全丢 / pseudo header 全丢 / passthrough 短路 / inline values 展开 / 多行 header。
- 复核 diff：`handlers/replay.rs`、`request_rules.rs`、E2E 用例、human_tests。
- 重点 review：
  - `host_changed` 判定：端口相同但 host 不同、host 相同但端口不同的边界。
  - 多行 header 里 `\r\n` 与 `\n` 混用是否都能拆分。
- 复测：
  - `cargo test -p bifrost-admin replay -- --nocapture`
  - `bash e2e-tests/tests/test_replay_rules.sh test_forward_localhost_api_rule`

### 第 2 轮

- 复核修复。
- 覆盖 SSE replay 走同一 forward header 路径。
- 覆盖 body 为 chunked 时 `Content-Length` 丢弃后 reqwest 生成正确长度。

## 风险与决策点

- **风险**：某些应用依赖客户端手动设置的 `Host`（例如浏览器测试）。当规则未改写 URL authority 时必须保留用户 Host。`host_changed=false` 分支已覆盖。
- **风险**：hop-by-hop 头列表未来若扩展（HTTP/3 相关头），需在同一常量维护。
- **决策**：不改 WebSocket replay 头过滤逻辑，避免影响握手；WS 有独立分支已经处理 hop-by-hop。
- **决策**：passthrough 短路是硬阻断，不与后续规则做“并列”；这符合普通代理链路语义。
- **决策**：inline values 展开与主端口共享同一解析器，避免出现两份行为分歧。
