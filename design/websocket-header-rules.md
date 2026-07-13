# WebSocket 握手头部规则设计方案

## 背景

Bifrost 的头部规则（`reqHeaders`、`resHeaders`、`delete_req_headers`、`delete_res_headers`、`ua`、`referer`、`auth`、`headerReplace`）在普通 HTTP/HTTPS 请求上生效，但 WebSocket 升级链路有两条完全独立的代码路径：

- 普通 `http://` 代理入口在识别 `Upgrade: websocket` 后由 `crates/bifrost-proxy/src/proxy/http/handler.rs` 调用 `websocket::handle_http_websocket`，不再进入普通 HTTP 请求 transform。
- TLS intercept `https://` 拆包后的 WSS 走 `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs` 的 `build_websocket_handshake_request`，也是独立握手构造。
- Replay 模块直接作为独立 WebSocket 客户端发起握手 (`crates/bifrost-admin/src/replay_executor.rs` / `replay_scripts.rs`)，规则通过 `ResolvedRules` 由 Admin 侧再解析一次。

如果不把 header 类规则显式挂到握手上，用户配置的 `reqHeaders://(X-Bifrost-E2E-WS-Request: injected)` 会在 WebSocket URL 上完全静默失效，且 traffic detail 里也没有原始 vs 最终头部对比。

本方案定义握手专用 header 规则的作用范围、执行位置、非破坏边界与真实测试证据。

## 用户目标验证清单

### 必须实现

- 下游客户端发起 `ws://` 或 TLS 解包后的 `wss://` 请求时，代理在发往远端前应用请求侧 header 规则。
- 远端返回 `101 Switching Protocols`（HTTP/1.1）或 H2 extended CONNECT 的 upgrade response 后，代理在返回客户端前应用响应侧 header 规则。
- Replay 模块作为独立 WS 客户端时，同样先应用请求/响应侧 header 规则再发起握手/回放。
- WebSocket 升级成功后的 frame 双向转发路径不改变 payload、不改变 header。
- Traffic 记录同时保留握手原始头 (`original_request_headers` / `original_response_headers`) 和最终头 (`request_headers` / `response_headers`)，供 UI 展示 diff 与诊断。

### 必须不破坏

- 已有 WebSocket 协议协商（`Upgrade`、`Connection`、`Sec-WebSocket-Accept`、`Sec-WebSocket-Protocol`、`Sec-WebSocket-Extensions`）仍由现有 `crates/bifrost-proxy/src/protocol/websocket/handshake.rs` 中的 `build_websocket_request_headers` / `build_websocket_response_headers` 负责。
- 普通 HTTP body/status/cache/attachment/redirect 规则不得套到 WebSocket 101 响应。
- permessage-deflate 协商、连接监控、frame 捕获、连接超时、`websocket_handshake_max_header_size` 兜底不受影响。
- Traffic 记录里 WebSocket 会话仍显示 matched rules、has_rule_hit。

### 必须真实验证

- 单元/集成测试证明普通 WS 和 TLS intercept WSS 两条路径均执行到 header 注入。
- E2E 脚本以真实 mock WS server 验证 mock 端确实收到注入的请求头、客户端确实收到注入的响应头。
- Replay 场景下 header 规则真实进入握手，`rule_config` 中 `resHeaders://(...)` 不会被 status/body 规则误伤。

## 产品语义

### 握手是一次 HTTP，规则作用点固定在握手

WebSocket 升级请求在物理上是一次 HTTP/1.1 GET 或 HTTP/2 extended CONNECT。Bifrost 把 header 类规则挂在这次握手的两个方向上：

- 请求侧：`req_headers`、`delete_req_headers`、`ua`、`referer`、`auth`、request `headerReplace`
- 响应侧：`res_headers`、`delete_res_headers`、response `headerReplace`

握手之后进入 frame 转发循环，Bifrost 只做 permessage-deflate 解压 (`crates/bifrost-proxy/src/protocol/websocket/deflate.rs`) 与可选 decode 脚本 (`decode://`)，不再修改 header 或 payload。

### body/status 类规则被明确排除

`status`、`resBody`、`reqBody`、`html.append`、`decode` 之外的 body 变换规则不得作用在 101 响应上；否则会破坏协议升级。因此握手侧 helper 只使用 header/URL/host 相关子集：

```rust
// 概念伪代码
pub fn apply_websocket_handshake_request_headers(
    parts: &mut RequestParts,
    rules: &ResolvedRules,
) { /* ua/referer/auth/delete_req_headers/req_headers/headerReplace */ }

pub fn apply_websocket_handshake_response_headers(
    parts: &mut ResponseParts,
    rules: &ResolvedRules,
) { /* delete_res_headers/res_headers/headerReplace */ }
```

### Traffic 与 matched rules 可诊断

`upgrade.rs` 与 tunnel 的 WSS 分支均写入 `record.request_headers = Some(req_headers.clone())`、`record.has_rule_hit = has_rules`、`record.matched_rules = build_matched_rules(&resolved_rules)`。前端 TrafficDetail Overview + Headers 面板可以像普通 HTTP 一样展示 original vs final header。

## 技术细节

### 普通 `ws://` 分支

- 入口：`crates/bifrost-proxy/src/proxy/http/handler.rs` 识别 upgrade 后调用 `crates/bifrost-proxy/src/proxy/http/websocket/mod.rs::handle_http_websocket`。
- 关键点：
  - `let resolved_rules = rules.resolve(&url, "GET");` 在建立 upstream 连接之前先解析。
  - `let req_headers: Vec<(String, String)> = crate::proxy::http::headers_to_pairs(req.headers());` 收集 original headers 用作 traffic 记录。
  - `build_websocket_handshake(&req)` 构造发往远端的 HTTP/1.1 GET；在生成 raw 前调用 request-side header 应用 helper（现有代码路径在此处已经预留 `resolved_rules` 与 `req` 引用）。
  - 响应侧：读取 `upstream_resp` 后，先应用 response header helper，再拷贝到最终 client-facing response builder（`response = response.header(...)`），仍保留 `Sec-WebSocket-Accept/Protocol/Extensions` 协商结果。
- 上限保护：`websocket_handshake_max_header_size`（默认 `64 * 1024`，见 `crates/bifrost-storage/src/unified_config.rs:171` 与 `crates/bifrost-proxy/src/server.rs:180`）继续对握手响应做上限保护，避免恶意远端注入超大响应头。

### TLS intercept `wss://` 分支

- 入口：`crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs::5085` 前后，识别 `is_websocket_upgrade(&req_headers_for_h3)` 后进入 `build_websocket_handshake_request(&req, &target_host, target_port, &target_path)`。
- 关键点：
  - `has_interceptable_fields` 判断包含 `req_headers/res_headers/delete_req_headers/delete_res_headers`，用来早退优化非规则场景，不影响握手 header 规则。
  - Request-side header helper 与 `handle_http_websocket` 使用同一份实现，只是入参 parts 类型不同。
  - Response-side 读取 upstream `101` 后走同一 helper，然后写回 client TLS stream。
  - traffic 记录同样透过 `record.request_headers` / `record.original_request_headers` / `record.response_headers` / `record.original_response_headers` 表达 diff。

### Replay WebSocket 分支

- 入口：`crates/bifrost-admin/src/replay_executor.rs` 与 `crates/bifrost-admin/src/replay_scripts.rs` 组合出 `rule_config`（Admin 侧再解析一次）。
- 请求侧规则原本已经生效；本设计要求响应侧也补齐 header 规则。
- 响应侧仅允许 header 规则（`resHeaders`/`delete_res_headers`/`headerReplace`），`statusCode` 与 body 规则在 Replay WS 响应上必须被忽略，且需要在 `crates/bifrost-admin/src/replay_response_rules.rs` 里显式短路，同时更新 Replay Traffic 记录的原始 / 最终响应头。

### 并发与热更新

- `RulesResolver` 是 `Arc<dyn>`，握手时 clone 一次结果 `resolved_rules`；升级完成后转入 frame 循环，即使规则热更新也不会撕裂正在进行的握手。
- frame 循环启动后传入 `ws_rules` / `ws_decode_scripts`，仅用于 decode 脚本，header 规则不再复用。

## CLI + Web + Admin API 边界

### CLI

本次不新增 CLI 参数或 subcommand。用户仍通过 `bifrost rule update <name> --content ...` 或 `--file` 写入规则；写入的 `ws://`、`wss://` 匹配器与既有语法一致。

### Web UI

- TrafficDetail Overview / Headers 面板显示原始 vs 最终 header diff；WebSocket 会话与普通 HTTP 请求共用组件。
- Rules Editor 无需新增字段。Monaco snippet (`web/src/components/BifrostEditor/snippet/protocol-docs.ts`) 中 `ws://` / `wss://` 匹配器已可直接书写 `reqHeaders://(...)` / `resHeaders://(...)`。

### Admin API

- `GET /_bifrost/api/traffic/:id` 返回的 WebSocket 会话字段与普通 HTTP 一致：`request_headers`、`original_request_headers`、`response_headers`、`original_response_headers`、`matched_rules`、`has_rule_hit`。
- `POST /_bifrost/api/replay/websocket` 的 `rule_config` 支持响应侧 header 规则；`replay_response_rules::apply_replay_response_rules` 明确忽略 body/status。

## Sync 边界

- 头部规则完全由用户规则文件承载，Sync 复用现有规则 Sync 路径，不需要新增字段。
- Group Sync 中携带 `ws://` / `wss://` 规则时行为与普通规则完全一致；不需要为握手 header 建立单独的 sync 通道。

## Phase 拆分

### Phase 1：握手专用 helper

- 抽出 `apply_websocket_handshake_request_headers` / `apply_websocket_handshake_response_headers`。
- 单元测试覆盖 `delete/insert/replace` 各种组合和 case-insensitive 名字匹配。
- 明确 body/status 规则的短路。

### Phase 2：普通 WS 路径接入

- `crates/bifrost-proxy/src/proxy/http/websocket/mod.rs` 在 `build_websocket_handshake` 前后各调用一次 helper。
- 写入 traffic 的 original vs final header。
- 补集成测试：`tests/https_proxy_test.rs::test_http_websocket_applies_request_and_response_header_rules`。

### Phase 3：TLS intercept WSS 路径接入

- `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs::build_websocket_handshake_request` 前后接入 helper。
- 补集成测试：`tests/https_proxy_test.rs::test_https_interception_websocket_applies_request_and_response_header_rules`。
- CI Windows 单独串行：`.github/workflows/ci.yml` 已把这条用例从 `cargo test --workspace` 并发中拆出，避免 TLS 握手夹具与同文件其它网络测试并发抖动。

### Phase 4：Replay WSS 补齐 + 文档 + human_tests

- `crates/bifrost-admin/src/replay_response_rules.rs` 增加 `replay_websocket_response_rules_apply_headers_only` 测试。
- 更新 `human_tests/proxy-websocket-sse.md::TC-PWS-09` 与 `human_tests/webui-replay.md` 中 Replay WS 规则头回归章节。
- 更新 `human_tests/readme.md` 用例数量。

## 测试方案

### 单元 / 集成测试

- `tests/https_proxy_test.rs::test_http_websocket_applies_request_and_response_header_rules`：普通 `ws://` 通过代理时，上游 mock server 收到 `X-Bifrost-WS-Request: injected`，客户端收到 `X-Bifrost-WS-Response: injected`，原始 `X-Upstream-WS: seen` 仍透传。
- `tests/https_proxy_test.rs::test_https_interception_websocket_applies_request_and_response_header_rules`：TLS intercept `wss://` 上游 TLS mock 收到 `X-Bifrost-WSS-Request: injected`，客户端收到 `X-Bifrost-WSS-Response: injected`，`X-Upstream-WSS: seen` 仍透传。
- `crates/bifrost-admin/src/replay_response_rules.rs::replay_websocket_response_rules_apply_headers_only`：Replay WS 响应只应用握手头规则；`statusCode` / body 规则被短路，upgrade 不被破坏。
- `crates/bifrost-proxy/tests/protocol_e2e.rs::test_websocket_handshake_and_echo`：作为握手基线回归，验证 helper 不破坏协议协商。

### E2E 测试

- `e2e-tests/tests/test_websocket_frames.sh`：加载 `e2e-tests/rules/websocket/header_rules.txt`（`ws://127.0.0.1:__WS_PORT__/ws/header-rules reqHeaders://(X-Bifrost-E2E-WS-Request: injected) resHeaders://(X-Bifrost-E2E-WS-Response: injected)`），断言：
  - mock WS server 日志出现 `X-Bifrost-E2E-WS-Request: injected`（脚本第 334 行）。
  - 客户端 upgrade response 中 `X-Bifrost-E2E-WS-Response=injected`（脚本第 326 行）。
  - Admin API traffic 记录中 `request_headers` 出现注入头（脚本第 365 行）。
- `e2e-tests/tests/test_replay_websocket_frames.sh`：覆盖 Replay WS `rule_config` 下的请求/响应头规则以及 body/status 规则的短路。

### 真实场景测试（human_tests）

- 更新并执行 `human_tests/proxy-websocket-sse.md::TC-PWS-09`（第 226 行）：
  - 走 `BIFROST_BIN=target/debug/bifrost bash e2e-tests/tests/test_websocket_frames.sh` 完成 mock WS 真实握手，`Passed: 6 / Failed: 0`。
  - 分别以 `cargo test --test https_proxy_test test_http_websocket_applies_request_and_response_header_rules -- --nocapture` 和 `cargo test --test https_proxy_test test_https_interception_websocket_applies_request_and_response_header_rules -- --nocapture` 复跑真实 TCP/TLS 用例。
- 更新并执行 `human_tests/webui-replay.md` Replay WebSocket 规则头回归章节。

## Review / Fix / Test 闭环

### 第 1 轮

- 复查用户目标：普通 WS、TLS intercept WSS、Replay WSS 三条路径均执行 header 规则；body/status 规则不套到 101。
- 复查 diff：`upgrade.rs`、`tunnel/mod.rs`、`replay_response_rules.rs`、`replay_executor.rs`、`design/websocket-header-rules.md`、`human_tests/proxy-websocket-sse.md`、`human_tests/webui-replay.md`、`human_tests/readme.md`。
- 复测定向：
  - `cargo test --test https_proxy_test test_http_websocket_applies_request_and_response_header_rules`
  - `cargo test --test https_proxy_test test_https_interception_websocket_applies_request_and_response_header_rules`
  - `cargo test -p bifrost-admin replay_websocket_response_rules_apply_headers_only`
  - `bash e2e-tests/tests/test_websocket_frames.sh`

### 第 2 轮

- 复查第 1 轮修复后的 diff，确认握手 helper 没有意外访问 body/status。
- 检查 CI Windows shard：`.github/workflows/ci.yml:1373-1374` 确认 `test_https_interception_websocket_applies_request_and_response_header_rules` 仍被 `--skip` 从并发批中排除并单独串行运行。
- 复跑受影响 human_tests、E2E 与对照的普通 HTTP 头注入用例（例如 `bash e2e-tests/tests/test_header_replace.sh`），确认非 WebSocket 路径未被破坏。

## 校验要求

- 先执行 WebSocket 头部规则定向测试（Phase 2 / Phase 3 / Phase 4 三条）。
- 再执行普通 HTTP 头部注入对照测试：`bash e2e-tests/tests/test_header_replace.sh`、`bash e2e-tests/tests/test_body_replace.sh`，确认未破坏非 WebSocket 路径。
- 收尾按 `rust-project-validate` 技能执行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`；如环境或耗时阻塞，必须记录风险。
- 本机遵循 no-local-coverage 约定时不再本地跑 `make coverage`；交付时说明豁免并依赖远端 CI。

## 文档更新要求

- 更新 `human_tests/proxy-websocket-sse.md::TC-PWS-09`。
- 更新 `human_tests/webui-replay.md` Replay WS 规则头回归段落。
- 更新 `human_tests/readme.md` 用例数量与说明。
- 本次不新增 CLI 参数或用户可见配置项，README/site 文档无需更新。

## 风险与决策点

- **握手响应体不可修改**：若未来有人希望在 101 响应上塞 body（例如异常降级页面），仍应统一走普通 HTTP 分支返回 4xx/5xx，不要给握手 helper 增加 body 修改能力。
- **HTTP/2 extended CONNECT**：当前 tunnel 路径已经处理 H2 extended CONNECT 的 WebSocket；helper 与 HTTP/1.1 完全共享，避免在 H2 上重复实现。
- **`websocket_handshake_max_header_size` 与规则注入的关系**：默认 64 KiB，若用户批量注入超大 header 需要提高该阈值；单元测试固化默认值（`crates/bifrost-storage/src/config_manager.rs:738/1145`），不建议自动放宽。
- **Windows CI 并发抖动**：`test_https_interception_websocket_applies_request_and_response_header_rules` 在 Windows 上必须单独 `--test-threads=1` 运行（`.github/workflows/ci.yml:1373-1374`）。回归时不要合并回并发批。
- **Replay 与 Proxy 双份 rule 解析**：Admin Replay 会重新解析一次 rule 内容，helper 需要保持与 Proxy 侧一致的 `case-insensitive` header 合并策略，避免出现 “Proxy 命中但 Replay 遗漏” 的诊断差异。
