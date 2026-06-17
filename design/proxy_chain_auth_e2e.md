# 双 Bifrost 代理链路鉴权 E2E 方案

## 功能模块描述

为 `proxy://` 路由补充一个真实端到端场景：启动两个独立的 Bifrost 代理服务，入口代理通过 `proxy://user:pass@127.0.0.1:<upstream_port>` 将请求转发到上游代理，上游代理再按 `host://` 规则转发到最终 mock 服务。

目标是覆盖两点：

- `proxy://` 规则可以把 HTTP 请求真正转发到另一个 Bifrost 代理
- `proxy://` 中配置的用户名密码会被编码并作为上游代理鉴权头带出
- 黑盒 shell E2E 能以脚本方式稳定复现上述链路

## 实现逻辑

- 在 `crates/bifrost-proxy` 的 HTTP 请求处理链路中，为 `resolved_rules.proxy` 增加专门的“上游 HTTP 代理”发送路径
  - 关键 helper 位于 `crates/bifrost-proxy/src/proxy/http/handler.rs`：`build_upstream_proxy_auth_value`、`build_upstream_proxy_connect_request`、`connect_via_upstream_http_proxy_tunnel`
  - HTTPS 隧道路径由 `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs` 调用 `connect_via_upstream_http_proxy_tunnel`；SOCKS5 升级路径在 `crates/bifrost-proxy/src/proxy/socks/tcp.rs` 同样复用
- 该路径直接连接 `proxy://` 指定的代理地址，并以 absolute-form 请求行把原始目标 URL 发给上游代理
- 当 `proxy://` 带有 `user:pass@` 时，构造 `Proxy-Authorization: Basic ...` 请求头（`base64(STANDARD)` 编码 `user:pass`）
- 在 `crates/bifrost-e2e/src/tests/routing.rs` 中新增独立测试 `test_routing_proxy_chain_with_auth`（注册名 `routing_proxy_chain_with_auth`，分类 `routing`）：
  - 启动 `ProxyEchoServer`（位于 `crates/bifrost-e2e/src/mock.rs`）作为最终 mock 服务
  - 启动上游 Bifrost，配置 `chain.test host://127.0.0.1:<mock_port>`
  - 启动入口 Bifrost，配置 `chain.test proxy://user:pass@127.0.0.1:<upstream_port>`
  - 通过入口代理请求 `http://chain.test/api?via=entry`
  - 断言最终响应成功、body 包含 `proxy_chain_ok`、`proxy-authorization` 头为 `Basic dXNlcjpwYXNz`，且 query 透传 `via=entry`
- 在 `e2e-tests/tests/test_proxy_chain_auth_e2e.sh` 新增 shell E2E：
  - 直接启动 release 版 `bifrost` 二进制（`target/release/bifrost`，或 `.exe`），分别使用独立 `BIFROST_DATA_DIR`
  - 所有下游服务都使用本机 mock：入口 Bifrost、上游 Bifrost、HTTP echo 与 proxy echo 都绑定在 `127.0.0.1`，不得依赖公网或 CI runner 的外部网络可达性。
  - 端口通过 `pick_available_base_port` 选择连续动态端口段，避免 macOS/Linux shell shard 并发时使用 `$$ % 200` 这类窄窗口导致本地下游 mock 或上游 Bifrost 被端口碰撞污染。
  - 使用规则夹具 `e2e-tests/rules/forwarding/proxy_chain_entry_auth.txt` 与 `proxy_chain_upstream_host.txt`，通过 `render_rule_fixture_to_file` 渲染端口占位符
  - 启动 Python mock：`e2e-tests/mock_servers/http_echo_server.py`（最终 echo）与 `e2e-tests/mock_servers/proxy_echo_server.py`（专门校验 `Proxy-Authorization`）
  - 一个脚本同时覆盖 `test_bifrost_proxy_chain`（双 Bifrost 代理链路）与 `test_downstream_proxy_auth`（下游代理鉴权 absolute-form + `Proxy-Authorization: Basic dXNlcjpwYXNz`）
  - 如果双代理链路返回非 2xx，脚本必须输出状态码、响应头、响应体，以及 entry/upstream/mock 日志尾部，方便一次性判断是端口冲突、上游进程异常、规则加载问题还是产品回归。

## 依赖项

- `crates/bifrost-proxy` 现有 HTTP 代理处理逻辑（`src/proxy/http/handler.rs`、`src/proxy/http/tunnel/mod.rs`）
- `crates/bifrost-e2e` 的 `ProxyInstance`、`CurlCommand`、`EnhancedMockServer`、`ProxyEchoServer`（见 `crates/bifrost-e2e/src/mock.rs`）
- `e2e-tests/test_utils/{assert,process,rule_fixture}.sh` shell 框架、规则夹具渲染工具、进程管理工具
- `e2e-tests/mock_servers/proxy_echo_server.py` 与 `http_echo_server.py`
- `e2e-tests/rules/forwarding/proxy_chain_entry_auth.txt` 与 `proxy_chain_upstream_host.txt` 规则夹具
- `tokio` 现有 TCP 连接能力（CONNECT 握手通过原生 `TcpStream` 完成，未走 `hyper` 客户端）

## 测试方案

- 新增 `routing` 分类 Rust E2E：验证双代理链路与鉴权头透传
- 新增 shell E2E：验证 release 二进制、规则文件渲染、双代理进程启动、代理链路与下游鉴权
- 单独执行目标测试，避免一次跑全量
- 补充局部单元测试，校验 `proxy://user:pass@host:port` 解析逻辑

## 校验要求

- 先执行目标 E2E：
  - `cargo run -p bifrost-e2e -- --test routing_proxy_chain_with_auth`
  - `bash e2e-tests/tests/test_proxy_chain_auth_e2e.sh`
- 再执行项目校验：
  - `cargo test --workspace --all-features`
  - `rust-project-validate`

## 文档更新要求

- 本次为测试覆盖与实现补齐，不新增外部配置项
- 若实现语义与现有 `docs/rules/routing.md` 中“代理认证”描述不一致，再同步更新规则文档

## 实现状态校对（截至 2026-06-16）

- Rust E2E `routing_proxy_chain_with_auth` 已落地：`crates/bifrost-e2e/src/tests/routing.rs:486`（注册位置 `crates/bifrost-e2e/src/tests/routing.rs:97`）
- Shell E2E `test_proxy_chain_auth_e2e.sh` 已落地：`e2e-tests/tests/test_proxy_chain_auth_e2e.sh`，已纳入 `scripts/run_all_e2e.sh` 与 `design/ci-shell-e2e.md` 的 shard
- 上游代理 helper 已实现：`build_upstream_proxy_auth_value` / `build_upstream_proxy_connect_request` / `connect_via_upstream_http_proxy_tunnel` 位于 `crates/bifrost-proxy/src/proxy/http/handler.rs`，对应单测 `test_build_upstream_proxy_auth_value`
- `proxy_echo_server.py` 与 `http_echo_server.py` 已在 `e2e-tests/mock_servers/` 下提供
- 规则夹具 `proxy_chain_entry_auth.txt` / `proxy_chain_upstream_host.txt` 已位于 `e2e-tests/rules/forwarding/`
- 关联人测脚本：`human_tests/ci-shell-e2e-sharding.md` 已记录该用例的覆盖证据
