# TLS MITM HTTP/2 WebSocket 兼容

## 背景

在 TLS 解包(MITM)场景下,浏览器和 Bifrost 之间通过 ALPN 通常协商到 `h2`。此前的 MITM HTTP/2 server builder 未启用 extended CONNECT,浏览器发起的 WebSocket 握手 (`h2` 下的 `CONNECT + :protocol = websocket`) 无法被代理识别,导致 `wss://` 连接失败。作为兼容性修复,方案要求在 MITM 通道上打开 HTTP/2 extended CONNECT,并把 H2 与 HTTP/1.1 两类 WebSocket 请求纳入同一条拦截链路,而不是通过降级 ALPN 到 `http/1.1` 规避问题。

同时上游服务可能仍然是 HTTP/1.1 的经典 `Upgrade: websocket`,代理需要跨版本转换: 从 H2 CONNECT 收帧,转成 HTTP/1.1 `Sec-WebSocket-Key` 握手投递给上游,再把 `200` 响应回给下游(下游是 H2 语义,不返回 `101`),同时保留 `Sec-WebSocket-Protocol` / `Sec-WebSocket-Extensions` 的协商结果。

## 用户目标验证清单

### 必须实现

- MITM `hyper-util` HTTP/2 builder 启用 `enable_connect_protocol()`,允许浏览器发起 H2 extended CONNECT。
- 拦截入口 `is_websocket_upgrade_request` 同时识别:
  - HTTP/1.1 `Upgrade: websocket` + `Connection: upgrade`
  - HTTP/2 `CONNECT` + `hyper::ext::Protocol == "websocket"`
- 非 TLS HTTP 转发入口也必须复用同一识别,避免 H2 extended CONNECT 被误判成普通 HTTP 请求,复用错误的上游 H2 连接池。
- H2 WebSocket 与 HTTP/1.1 WebSocket 复用同一套握手转发、连接监控、帧捕获与 traffic 记录逻辑。
- 上游仍是 HTTP/1.1 WebSocket 时,代理为上游补齐 `Sec-WebSocket-Key`、返回 `200` 给下游(不返回 `101`)、保留 `Sec-WebSocket-Protocol`/`Sec-WebSocket-Extensions`。

### 必须不破坏

- 现有 HTTP/1.1 WebSocket 握手、帧捕获、`ws_decode` 存储路径不变。
- SSE、普通 HTTP/2 请求不被识别为 WebSocket,不误进 upgrade 路径。
- Bifrost -> 上游 HTTP/2 连接池的复用逻辑对普通请求保持不变。
- HTTP/3 上游/非拦截路径仍能识别 WebSocket 并回退到 HTTP/2 或 HTTP/1.1(H3 不支持 CONNECT-UDP WebSocket 时降级)。
- 已有 rules/scripts/replay 对 WebSocket 帧的处理保持一致。

### 必须真实验证

- 使用真实 Chrome 打开 `wss://echo.test/` 通过 Bifrost MITM,断言 ALPN 协商为 `h2`,WebSocket 文本帧双向可达。
- Firefox 同样验证。
- 上游为 HTTP/1.1 WebSocket 时,下游 H2 客户端仍可收发帧。

## 产品语义

`is_websocket_upgrade_request` 是拦截入口的唯一 WebSocket 判定函数:

```rust
fn is_websocket_upgrade_request(req: &Request<Incoming>) -> bool {
    if req.version() == hyper::Version::HTTP_2
        && req.method() == hyper::Method::CONNECT
        && req
            .extensions()
            .get::<hyper::ext::Protocol>()
            .is_some_and(|protocol| protocol.as_str().eq_ignore_ascii_case("websocket"))
    {
        return true;
    }

    let connection = req.headers().get(hyper::header::CONNECTION)
        .and_then(|v| v.to_str().ok()).unwrap_or("");
    let upgrade = req.headers().get(hyper::header::UPGRADE)
        .and_then(|v| v.to_str().ok()).unwrap_or("");
    connection.to_lowercase().contains("upgrade") && upgrade.to_lowercase() == "websocket"
}
```

任一分支命中即视为 WebSocket 升级请求,后续走 `handle_intercepted_websocket`。

## 技术细节

### MITM HTTP/2 builder

`crates/bifrost-proxy/src/server.rs` 构造 MITM 服务器时:

```rust
builder
    .http2()
    .adaptive_window(true)
    .enable_connect_protocol()
    // ...其它 header 限额调整
```

`enable_connect_protocol()` 会在 `SETTINGS` 帧中广播 `SETTINGS_ENABLE_CONNECT_PROTOCOL=1`,浏览器据此允许对同一连接发起 `CONNECT + :protocol=websocket`。

### 请求分派

`crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs`:

```rust
if is_websocket_upgrade_request(&req) {
    return handle_intercepted_websocket(req, original_host, /* ... */).await;
}
```

同一函数用于:

- HTTP CONNECT MITM 入口
- SOCKS5 TLS MITM 入口(通过 `unified.rs` 复用)
- 非 TLS 转发入口(`handler.rs`),防止 H2 extended CONNECT 被当作普通 CONNECT 走到上游 H2 池。

### H2 到 HTTP/1.1 上游桥接

`crates/bifrost-proxy/src/proxy/http/websocket/upgrade.rs`:

- 从下游 H2 CONNECT 帧中提取 `:protocol`、`Sec-WebSocket-Protocol`、`Sec-WebSocket-Extensions`。
- 使用 `rand`/`base64` 生成 16 字节随机 `Sec-WebSocket-Key`,拼接 HTTP/1.1 握手行:

  ```text
  GET {path} HTTP/1.1\r\n
  Host: {host}\r\n
  Upgrade: websocket\r\n
  Connection: Upgrade\r\n
  Sec-WebSocket-Key: {generated}\r\n
  Sec-WebSocket-Version: 13\r\n
  ...(转发 Sec-WebSocket-Protocol / Sec-WebSocket-Extensions)\r\n\r\n
  ```

- 上游返回 `101 Switching Protocols` 后:
  - 校验 `Sec-WebSocket-Accept`(可选,用于诊断)。
  - 对下游 H2 CONNECT 回应 `:status = 200`,而不是 `101`。
  - 把上游协商的 `Sec-WebSocket-Protocol` / `Sec-WebSocket-Extensions` 透传到下游 H2 header。
- 之后进入 `websocket/capture.rs` 与 `ws_decode` 记录帧,与 HTTP/1.1 路径共用。

### 帧压缩与扩展

- `parse_permessage_deflate_config` 复用现有逻辑,H2 WebSocket 同样根据协商结果启用压缩。
- 帧存储 `WsHandshakeMeta` 记录 negotiated protocol/extensions,traffic UI 能显示。

## CLI + Web + Admin API

- 无新增 CLI 参数、无新增 Admin API。
- Traffic 记录中 WebSocket 会带 `protocol=websocket`、`transport=h2/h1`,Web UI 可展示帧列表。
- `bifrost traffic get <id>` 保持一致输出,H2 WebSocket 与 H1 WebSocket 在同一 record 类型下。

## Sync 边界

- 本次修复不引入新配置,无 sync payload 变更。
- `enable_tls_interception` 与规则的 sync 边界保持不变。

## Phase 1: MITM builder 打开 extended CONNECT

- 在 `server.rs` 的 HTTP/2 builder 上调用 `enable_connect_protocol()`。
- 补充单元测试或 integration test,验证 SETTINGS 帧包含 `ENABLE_CONNECT_PROTOCOL`。

## Phase 2: 请求识别统一

- 抽出 `is_websocket_upgrade_request`,同时识别 H1 upgrade 与 H2 extended CONNECT。
- 在 tunnel/handler/socks 入口统一调用。
- 单测覆盖 true/false 分支。

## Phase 3: 上游 H1 兼容与响应改写

- `websocket/upgrade.rs` 支持 H2 -> H1 桥接: 生成 Sec-WebSocket-Key、返回 200 给 H2 客户端、透传协商结果。
- 帧路径继续走 `websocket/capture.rs` 与 `ws_decode`,H2 WebSocket 与 H1 共存。

## Phase 4: 兼容性验证与文档

- 使用真实 Chrome + `wss://echo.test`、Firefox 验证 ALPN 与帧收发。
- 补充 `human_tests/proxy-websocket-sse.md` 用例说明 H2 WebSocket 支持。
- 无需 README 变更(纯兼容性修复)。

## 测试方案

### 单元测试

- `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs`:
  - `test_is_websocket_upgrade_request_true_for_http1_headers_v4`
  - `test_is_websocket_upgrade_request_false_without_headers_v4`
  - 新增 `test_is_websocket_upgrade_request_true_for_http2_extended_connect`(H2 CONNECT + `:protocol=websocket`)。
- `crates/bifrost-proxy/src/proxy/http/websocket/upgrade.rs`: 覆盖 H2 -> H1 桥接的 Sec-WebSocket-Key 生成与响应改写。
- `crates/bifrost-proxy/src/protocol/websocket/handshake.rs`: 覆盖 extension 解析。

### 集成/E2E 测试

- `tests/https_proxy_test.rs`: 增加 H2 WebSocket 场景。
- `crates/bifrost-proxy/tests/protocol_e2e.rs`: 覆盖 `CONNECT -> TLS MITM -> H2 extended CONNECT websocket -> ws echo`。
- `e2e-tests/mock_servers/http_ws_echo_server.py` + `e2e-tests/mock_servers/ws_echo_server.py`: 已有 mock。
- `e2e-tests/test_utils/ws_stress_client.py`: 压测帧收发。
- `human_tests/proxy-websocket-sse.md`: 真实浏览器验证。

### 真实场景

- `wss://` echo 通过 Bifrost MITM: Chrome + Firefox 各验证一次。
- 上游改为 HTTP/1.1 upstream 时依然可收发文本/二进制帧。
- 上游 permessage-deflate 协商成功时压缩帧能被 traffic UI 展示。
- 使用 `--no-system-proxy`、临时数据目录、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 启动。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核: `enable_connect_protocol()` 是否影响非 WebSocket 的 H2 请求(不应有副作用)。
- 复核: `is_websocket_upgrade_request` 是否会误判普通 CONNECT(应仅在 H2 + `:protocol=websocket` 时返回 true)。
- 复测: `test_is_websocket_upgrade_request_*`、integration H2 WebSocket、真实浏览器 wss。

### 第 2 轮

- 检查 H2 -> H1 桥接时 header 是否漏传(尤其是 `Origin`、`Cookie`)。
- 检查 `ws_decode` 存储是否把 H2 帧和 H1 帧标记一致,避免 traffic detail 类型混乱。
- 复测 stress client、`cargo test --workspace --all-features`。

## 校验要求

- 先执行本模块 WebSocket / TLS 相关 E2E 与 integration test。
- 再执行 `cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features -- -D warnings`、按修改范围 `cargo test`、`cargo build --all-targets --all-features`(交由 CI 或本地手动)。
- 本地约定 no-local-coverage,不跑 `make coverage`。

## 风险与决策

- 决策: 不通过降级 `http/1.1` 规避 H2 WebSocket 问题。原因: 现代浏览器与 CDN 大量使用 H2,若强制 ALPN 只暴露 `http/1.1`,会破坏其它 H2 多路复用与优先级优化,回归成本远高于打开 extended CONNECT。
- 风险: `enable_connect_protocol()` 在旧版 `hyper-util` 可能不存在,构建失败。缓解: 更新依赖并在 Cargo.lock 中锁定版本。
- 风险: H2 -> H1 桥接生成的 `Sec-WebSocket-Key` 与上游返回 `Sec-WebSocket-Accept` 不匹配时,代理无法从校验角度发现问题(下游是 H2 语义,不校验 accept)。缓解: 在 debug log 输出握手结果,traffic detail 可看到握手 header 供排查。
- 风险: 上游若也支持 H2 WebSocket,当前实现仍走 H1 桥接。缓解: 保持行为一致,后续可扩展 H2 -> H2 直转,减少一次协议翻译。
- 风险: `permessage-deflate` 与 H2 CONNECT 组合的 header 位置(H2 帧 header 与 H1 大小写差异)需要显式统一,已在 `parse_permessage_deflate_config` 复用逻辑中处理。
