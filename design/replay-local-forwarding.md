# Replay Local Forwarding

## 问题描述

当 Replay 重放原始 HTTPS API 请求，并启用如下本地开发转发规则时：

```text
bifrost.local http://localhost:5173/
```

请求：

```text
https://bifrost.local/api/nextagent/v1/sessions
```

会先被规则改写为本地 HTTP 上游：

```text
http://localhost:5173/api/nextagent/v1/sessions
```

但 unified replay 发送请求时仍逐个转发原始请求头，旧的 `Host: bifrost.local` 以及 `Connection`、`Content-Length` 等传输级头会继续进入本地上游，导致本地 dev server / proxy 链路可能返回 502 或异常断开。

完整 nextoncall PPE 规则还包含更高优先级的 API 透传规则：

```text
https://bifrost.local/api/nextagent/ passthrough://
bifrost.local http://localhost:5173/
```

Replay 的规则应用层原先只把 `passthrough://` 记录为匹配规则，后续仍继续应用域名级 `http://localhost:5173/` 转发。因此 `/api/nextagent/` 本应透传到真实上游，却被错误转发到本地 Vite 服务，形成 502。

同时，Replay 从规则存储读取规则时直接使用普通 `parse_rules`，没有解析规则文件中的 markdown inline values。`reqHeaders://{ppe2}` 这种写法在 replay 路径里会保留成未展开占位符，无法实际注入 `x-tt-env` / `x-use-ppe`。

## 根因

普通代理转发链路会根据最终上游目标重新构造传输层请求头；Replay 的 unified HTTP 执行路径使用独立的 `reqwest` 请求构造逻辑，规则改写 URL 后没有过滤原始 transport headers。

Replay 规则重放还需要和普通代理保持同一套规则语义：

- `passthrough://` 命中后必须阻断后续转发类规则，不能再应用域名级 `http/https/ws/wss/host/xhost`。
- 规则文件中的 markdown inline values 必须参与 replay 规则解析。
- `reqHeaders` 展开成多行 header 时，需要拆成多个 header 注入请求。

需要区分两类头：

- 应用语义头：`Authorization`、`Content-Type`、业务自定义头等，应继续保留。
- 传输级头：`Host`、`Content-Length`、`Connection`、`Transfer-Encoding`、HTTP/2 pseudo headers 等，应由 HTTP client 根据最终 URL/body 重新生成。

## 修复方案

1. 在 `crates/bifrost-admin/src/handlers/replay.rs` 的 unified replay 路径增加 HTTP 转发头过滤。
2. 当原始 URL authority 与规则改写后的 URL authority 不一致时，丢弃旧 `Host`，让本地目标生成自己的 Host。
3. 始终丢弃 hop-by-hop / transport headers：
   - `Content-Length`
   - `Transfer-Encoding`
   - `Connection`
   - `Proxy-Connection`
   - `Keep-Alive`
   - `TE`
   - `Trailer`
   - `Upgrade`
   - HTTP/2 pseudo headers（如 `:authority`）
4. WebSocket replay 已有独立握手过滤逻辑，本次修复保持其行为不变。
5. 在 `request_rules` 中增加 `forwarding_passthrough` 状态；命中 `passthrough://` 后清空已记录的转发目标和 host 覆盖，并阻断后续转发类规则。
6. Replay 的自定义规则和存储规则解析统一改为 `RuleParser::parse_rules_with_inline_values`，并将 inline values 合并到 resolver value store。
7. `reqHeaders` 的 resolved value 支持多行解析，例如 `x-tt-env: ...\nx-use-ppe: ...` 会注入两个请求头。

## 测试方案

### 单元测试

- `crates/bifrost-admin/src/handlers/replay.rs`
  - `replay_forward_skips_stale_host_when_rule_changes_target`
  - `replay_forward_keeps_custom_host_when_authority_is_unchanged`
  - `replay_forward_skips_hop_by_hop_headers_and_pseudo_headers`
- `crates/bifrost-admin/src/request_rules.rs`
  - `test_passthrough_blocks_later_forward_rule_from_resolved_rules`
  - `test_parse_header_values_multiline`

### E2E 测试

- 更新 `e2e-tests/tests/test_replay_rules.sh`
  - 新增 `test_forward_localhost_api_rule`
  - 使用 `bifrost.local http://127.0.0.1:<mock>/` 规则重放 `/api/nextagent/v1/sessions`
  - 断言上游收到 `/api/nextagent/v1/sessions`
  - 断言旧 `Host: bifrost.local` 未泄漏到本地上游
  - 断言 `Connection` hop-by-hop 头未泄漏

### Human Tests

- 更新 `human_tests/webui-replay.md`
  - 更新 TC-WRP-23：Replay HTTPS API + localhost 转发规则回归
  - 增加完整 nextoncall PPE 规则：API passthrough 命中后不再被域名级 localhost 规则覆盖，且 PPE headers 已实际注入。

## 校验要求

- 先执行 replay E2E：`bash e2e-tests/tests/test_replay_rules.sh`
- 再执行 targeted Rust 测试：`cargo test -p bifrost-admin replay_body_decode_tests -- --nocapture`
- 最后执行项目门禁：
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  - `cargo test --workspace --all-features`

## 文档更新要求

- 更新 `human_tests/webui-replay.md`
- 更新 `human_tests/readme.md`
