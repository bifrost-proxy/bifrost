# 双 Bifrost 代理链路鉴权 E2E 方案

## 背景

`proxy://` 规则允许把请求转发到另一个上游 HTTP 代理，`proxy://user:pass@host:port` 语法进一步允许把用户名密码编码后作为 `Proxy-Authorization: Basic ...` 带出。此前虽然有单测覆盖 `build_upstream_proxy_auth_value` 的 Basic 编码逻辑，但没有真实端到端场景验证：一个 Bifrost 作为入口代理、另一个 Bifrost 作为上游代理、鉴权头在两跳之间正确传递、最终 mock 服务收到目标 URL 与 query。本方案补足这条黑盒链路，同时以 shell E2E 形式在 release 二进制上稳定复现，覆盖端口分配、规则夹具渲染、日志诊断等真实调试场景。

真实实现位置：

- Rust E2E：`crates/bifrost-e2e/src/tests/routing.rs`（`test_routing_proxy_chain_with_auth` 注册于 `:97`，实现于 `:486`）。
- Shell E2E：`e2e-tests/tests/test_proxy_chain_auth_e2e.sh`（已纳入 `scripts/run_all_e2e.sh` 与 `design/ci-shell-e2e.md` 的 shard）。
- 上游 helper：`crates/bifrost-proxy/src/proxy/http/handler.rs`（`build_upstream_proxy_auth_value:437`、`connect_via_upstream_http_proxy_tunnel:479`）。
- Mock 服务：`e2e-tests/mock_servers/proxy_echo_server.py`、`e2e-tests/mock_servers/http_echo_server.py`。
- 规则夹具：`e2e-tests/rules/forwarding/proxy_chain_entry_auth.txt`、`proxy_chain_upstream_host.txt`。

## 用户目标验证清单

### 必须实现

- 覆盖 `proxy://` 规则真正把 HTTP 请求转发到另一个 Bifrost 代理，且原始目标 URL 与 query 完整透传。
- 覆盖 `proxy://user:pass@host:port` 的 Basic 编码到 `Proxy-Authorization: Basic dXNlcjpwYXNz` header，与 `base64(STANDARD)` 编码一致。
- 覆盖 HTTP 明文与 HTTPS CONNECT 隧道两条链路：HTTPS 隧道路径由 `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs` 调用 `connect_via_upstream_http_proxy_tunnel`；SOCKS5 升级路径在 `crates/bifrost-proxy/src/proxy/socks/tcp.rs` 复用同一 helper。
- shell E2E 使用 release 二进制、独立 `BIFROST_DATA_DIR`、动态端口段、127.0.0.1-only 依赖，不依赖公网或 CI runner 外部网络。
- 一个 shell 脚本同时覆盖 `test_bifrost_proxy_chain`（双代理链路）与 `test_downstream_proxy_auth`（下游代理鉴权 absolute-form + `Proxy-Authorization: Basic`）。
- 失败时输出状态码、响应头、响应体、entry/upstream/mock 日志尾部，方便一次性判断端口冲突 / 上游进程异常 / 规则加载问题 / 产品回归。

### 必须不破坏

- `proxy://` 规则原语义：absolute-form 请求行、hop-by-hop header 处理、`Proxy-Authorization` 只发给上游代理不发给最终目标。
- `proxy_echo_server.py` 与 `http_echo_server.py` mock 语义：`raw_path`、`parsed_path`、`query_string`、`host`、`proxy-authorization` 字段。
- 现有 routing 分类其他用例继续可跑；新用例注册后不破坏 `--test <name>` 过滤。
- shell 脚本共用 `e2e-tests/test_utils/{assert,process,rule_fixture}.sh`，不引入独立辅助函数副本。

### 必须真实验证

- `cargo run -p bifrost-e2e -- --test routing_proxy_chain_with_auth` 单跑通过。
- `bash e2e-tests/tests/test_proxy_chain_auth_e2e.sh` 在 macOS / Linux 本机通过。
- `cargo test -p bifrost-proxy handler::test_build_upstream_proxy_auth_value -- --nocapture` 通过。
- `crates/bifrost-proxy/src/proxy/http/handler.rs:5457 connect_via_upstream_http_proxy_tunnel_succeeds_with_wiremock_proxy` 与 `:6301` non-http scheme rejection、`:6319` non-2xx failure、`:6649` no-credential、`:7487` username-only 分支单测通过。

## 产品语义

### `proxy://` 上游代理

- 请求行采用 absolute-form（`GET http://target/path?query HTTP/1.1`）发送到上游代理。
- `Proxy-Authorization` 仅在 `proxy://` 携带凭证时生成，且仅面向上游代理。
- 最终目标站点收到的请求应是从上游代理再转发出去的，不带下游客户端的 `Proxy-Authorization`（由下游 Bifrost 的 header 清洗保证）。
- HTTPS 隧道通过 `CONNECT host:port` 到上游代理，收到 2xx 后原样透传 TLS 字节；非 2xx 或非 http scheme 都视为失败。

### 测试链路

```text
curl ──HTTP──▶ [entry Bifrost :entry]
                proxy://user:pass@127.0.0.1:upstream
                        ──HTTP absolute-form + Basic dXNlcjpwYXNz──▶ [upstream Bifrost :upstream]
                                host://127.0.0.1:mock
                                        ──HTTP──▶ [http_echo mock :mock]
```

Proxy-only 校验链路：

```text
curl ──HTTP absolute-form + Basic──▶ [proxy_echo :proxy]
```

## 技术细节

### Rust helper（`crates/bifrost-proxy/src/proxy/http/handler.rs`）

```rust
pub(crate) fn build_upstream_proxy_auth_value(proxy_url: &Url) -> Option<String>;
pub(crate) async fn connect_via_upstream_http_proxy_tunnel(
    proxy_rule: &ResolvedProxy,
    host: &str,
    port: u16,
) -> Result<TcpStream, UpstreamProxyError>;
```

- `build_upstream_proxy_auth_value`：读取 URL 的 `username` / `password`，`base64::engine::general_purpose::STANDARD.encode("user:pass")`，返回 `Some("Basic <base64>")` 或 `None`。
- `connect_via_upstream_http_proxy_tunnel`：直接 `TcpStream::connect(proxy_addr)`，发送 `CONNECT host:port HTTP/1.1` + `Host` + 可选 `Proxy-Authorization`，解析响应行并要求 2xx。

### HTTP 明文路径

`handle_request()` 中 `resolved_rules.proxy` 触发上游 HTTP 代理路径：直接连接 `proxy://` 指定地址，构造 absolute-form 请求行 `GET http://chain.test/api?via=entry HTTP/1.1`，追加 `Proxy-Authorization: Basic dXNlcjpwYXNz`。

### HTTPS CONNECT 路径

`crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs` 复用 `connect_via_upstream_http_proxy_tunnel`：先与上游 Bifrost 建立 TCP，然后发 `CONNECT` + 可选 `Proxy-Authorization`，2xx 后把 TLS 字节直接双向 pipe。

### 规则夹具

`e2e-tests/rules/forwarding/proxy_chain_entry_auth.txt`：

```text
chain.test proxy://user:pass@127.0.0.1:{{upstream_port}}
```

`e2e-tests/rules/forwarding/proxy_chain_upstream_host.txt`：

```text
chain.test host://127.0.0.1:{{mock_port}}
```

通过 `render_rule_fixture_to_file`（`e2e-tests/test_utils/rule_fixture.sh`）渲染端口占位符，避免手写 sed。

### Mock 服务

- `proxy_echo_server.py`：模拟“上游代理”，回显 `raw_path`、`headers['proxy-authorization']`、`host`。
- `http_echo_server.py`：模拟“最终目标”，回显 `parsed_path`、`query_string`。

### 端口分配

Shell E2E 使用 `pick_available_base_port`（`e2e-tests/test_utils/process.sh`）选择连续动态端口段，避免 macOS/Linux shell shard 并发时 `$$ % 200` 类窄窗口引发的端口污染。

## CLI+Web+Admin API

本方案不新增 CLI/Web/Admin API 字段。相关 CLI/文档语义：

- `bifrost rule show <name>` 显示 `proxy://user:pass@host:port` 的规则内容；密码不脱敏（规则文件本身即明文，走 fs 权限保护）。
- `bifrost start --rule-file ./forwarding.bifrost` 支持加载规则文件。
- Admin API `GET /_bifrost/api/rules/<name>` 返回原始规则文本。

## Sync 边界

- 规则文件是否 sync 由用户在 Web 上勾选，与本方案无关；一旦 sync，密码会随明文规则同步到云端，用户需自己评估风险，本方案不额外加密。
- Rust/Shell E2E 强制使用临时 `BIFROST_DATA_DIR` + `--no-system-proxy`，不触发 sync。

## Phase 1 —— Rust helper + 单测（已 shipped）

- `build_upstream_proxy_auth_value` + `connect_via_upstream_http_proxy_tunnel` 落地。
- 单测：
  - `test_build_upstream_proxy_auth_value`（`:5142`）
  - `build_upstream_proxy_auth_value_is_none_without_credentials`（`:6649`）
  - `build_upstream_proxy_auth_value_handles_username_without_password`（`:7487`）
  - `connect_via_upstream_http_proxy_tunnel_succeeds_with_wiremock_proxy`（`:5457`）
  - `connect_via_upstream_http_proxy_tunnel_rejects_non_http_scheme`（`:6301`）
  - `connect_via_upstream_http_proxy_tunnel_fails_for_non_2xx_status`（`:6319`）

## Phase 2 —— Rust E2E `routing_proxy_chain_with_auth`（已 shipped）

`crates/bifrost-e2e/src/tests/routing.rs`：

- 启动 `ProxyEchoServer`（`crates/bifrost-e2e/src/mock.rs`）作为最终 mock。
- 启动上游 Bifrost，配置 `chain.test host://127.0.0.1:<mock_port>`。
- 启动入口 Bifrost，配置 `chain.test proxy://user:pass@127.0.0.1:<upstream_port>`。
- `CurlCommand` 发起 `http://chain.test/api?via=entry`。
- 断言：响应 2xx；body 含 `proxy_chain_ok`；`proxy-authorization` header 为 `Basic dXNlcjpwYXNz`；query `via=entry` 透传。
- 分类：`routing`；注册名：`routing_proxy_chain_with_auth`。

## Phase 3 —— Shell E2E `test_proxy_chain_auth_e2e.sh`（已 shipped）

`e2e-tests/tests/test_proxy_chain_auth_e2e.sh`：

- 使用 release `target/release/bifrost`（Windows 为 `.exe`），分别启动 entry / upstream Bifrost，独立 `BIFROST_DATA_DIR`。
- 依赖 mock：`http_echo_server.py`（最终 echo）、`proxy_echo_server.py`（校验 `Proxy-Authorization`）。
- 规则夹具：`proxy_chain_entry_auth.txt` + `proxy_chain_upstream_host.txt`，通过 `render_rule_fixture_to_file` 渲染端口。
- 端口通过 `pick_available_base_port` 分配连续段。
- 测试函数：
  - `test_bifrost_proxy_chain`（`:244`）
  - `test_downstream_proxy_auth`（`:255`）
- 断言：`assert_status_2xx`、`assert_body_contains '"parsed_path": "/chain"'`、`assert_body_contains '"query_string": "via=entry"'`、`assert_body_contains '"raw_path": "http://auth-proxy.test/auth?hello=1"'`、`assert_body_contains '"proxy-authorization": "Basic dXNlcjpwYXNz"'`、`assert_body_contains '"host": "auth-proxy.test"'`。
- 失败时输出：HTTP status、响应头、响应体、entry / upstream / mock 日志尾部（tail 50）。

## Phase 4 —— CI 分片 + 人测证据（已 shipped）

- `scripts/run_all_e2e.sh` 与 `design/ci-shell-e2e.md` shard 已纳入本脚本。
- `human_tests/ci-shell-e2e-sharding.md` 记录本用例覆盖证据。
- README/规则文档若与实现语义不一致，同步更新 `docs/rules/routing.md`。

## 测试方案

### 单元测试

- `cargo test -p bifrost-proxy proxy::http::handler::tests::test_build_upstream_proxy_auth_value`
- `cargo test -p bifrost-proxy proxy::http::handler::tests::build_upstream_proxy_auth_value_is_none_without_credentials`
- `cargo test -p bifrost-proxy proxy::http::handler::tests::build_upstream_proxy_auth_value_handles_username_without_password`
- `cargo test -p bifrost-proxy proxy::http::handler::tests::connect_via_upstream_http_proxy_tunnel_succeeds_with_wiremock_proxy`
- `cargo test -p bifrost-proxy proxy::http::handler::tests::connect_via_upstream_http_proxy_tunnel_rejects_non_http_scheme`
- `cargo test -p bifrost-proxy proxy::http::handler::tests::connect_via_upstream_http_proxy_tunnel_fails_for_non_2xx_status`

### E2E

- `cargo run -p bifrost-e2e -- --test routing_proxy_chain_with_auth`
- `bash e2e-tests/tests/test_proxy_chain_auth_e2e.sh`

### 真人回归

- `human_tests/ci-shell-e2e-sharding.md`：确认本脚本仍出现在 CI shard 输出中。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核端口分配是否使用 `pick_available_base_port`，绝不能用 `$$ % 200`。
- 复核 mock 服务只绑定 127.0.0.1，不监听 0.0.0.0。
- 复核规则夹具占位符渲染前后端口对齐。
- 复测：Rust E2E + Shell E2E + helper 单测。

### 第 2 轮

- 复核 HTTPS CONNECT 路径也复用同一 helper（`crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs`）。
- 复核 SOCKS5 升级路径 `crates/bifrost-proxy/src/proxy/socks/tcp.rs` 是否走同一 helper。
- 复核失败输出确实包含 entry/upstream/mock 日志尾部。
- 复测：`bash e2e-tests/tests/test_proxy_chain_auth_e2e.sh 2>&1 | tail -60` 观察诊断信息。

## 风险与决策

- **`proxy://user:pass@` 明文密码**：规则文件本身即明文；本方案不加密。若用户 sync 规则会把密码上云，需在 UI/文档警示。
- **端口冲突**：所有 mock/上游服务只绑 127.0.0.1；shell shard 并发用 `pick_available_base_port` 消除窄窗口冲突。
- **hop-by-hop header 泄露**：单元测试锁死 `Proxy-Authorization` 只发上游代理，不出现在最终 echo 服务收到的 header 中。
- **CONNECT 隧道错误码**：非 2xx / 非 http scheme 明确返回错误，避免静默走通导致后续 TLS 报错难以定位。
- **CI 稳定性**：release build 相比 debug 更接近用户视角；未来若引入 debug-only 优化路径，必须保证 shell E2E 仍能覆盖 release 行为。

## 校验要求

- `cargo test -p bifrost-proxy proxy::http::handler`
- `cargo run -p bifrost-e2e -- --test routing_proxy_chain_with_auth`
- `bash e2e-tests/tests/test_proxy_chain_auth_e2e.sh`
- `cargo test --workspace --all-features`
- `rust-project-validate`

## 文档更新要求

- 本次为测试覆盖与实现补齐，不新增外部配置项。
- 若实现语义与 `docs/rules/routing.md` 的“代理认证”描述不一致，同步更新规则文档。
- `human_tests/ci-shell-e2e-sharding.md` 与本方案保持同步。

## 实现状态校对（截至 2026-06-16）

- Rust E2E `routing_proxy_chain_with_auth`：`crates/bifrost-e2e/src/tests/routing.rs:486`（注册位置 `crates/bifrost-e2e/src/tests/routing.rs:97`）。
- Shell E2E `test_proxy_chain_auth_e2e.sh`：`e2e-tests/tests/test_proxy_chain_auth_e2e.sh`，已在 shard 中。
- 上游 helper：`build_upstream_proxy_auth_value` / `build_upstream_proxy_connect_request` / `connect_via_upstream_http_proxy_tunnel` 位于 `crates/bifrost-proxy/src/proxy/http/handler.rs`；对应单测 `test_build_upstream_proxy_auth_value` 已通过。
- Mock：`proxy_echo_server.py`、`http_echo_server.py` 已在 `e2e-tests/mock_servers/`。
- 规则夹具：`proxy_chain_entry_auth.txt` / `proxy_chain_upstream_host.txt` 已在 `e2e-tests/rules/forwarding/`。
- 人测证据：`human_tests/ci-shell-e2e-sharding.md` 已记录该用例。
