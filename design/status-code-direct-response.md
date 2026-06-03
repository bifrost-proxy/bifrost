# statusCode 直接响应优化

## 功能模块说明

`statusCode://code` 的语义是直接返回指定 HTTP 状态码，不向后端服务器发送请求。此前 HTTP 明文路径在未配置 `host` 时已经走 mock response，但 `host://... statusCode://code` 会先请求 upstream 再改响应状态码，和 `docs/rules/status-redirect.md` 中的规则说明不一致。

本优化将 `statusCode` 统一收敛为强制返回语义：只要命中 `statusCode`，且没有更具体的 mock file/template/rawfile/locationHref 响应载体，就在请求转发前构造响应并返回。

## 实现逻辑

- HTTP 明文代理路径复用 `generate_mock_response`，去掉 `statusCode` 对 `rules.host.is_none()` 的限制。
- HTTPS TLS 拦截路径在转发 upstream 前增加同等直接响应分支。
- `replaceStatus` 保持原语义：请求仍发送到后端，仅在响应返回后替换状态码。
- `resBody://...`、`resHeaders://...`、`resType://...`、`resCharset://...`、`cache://...` 继续可与 `statusCode` 组合，由直接响应构造器写入响应。
- `file://`、`tpl://`、`rawfile://` 等 mock 内容规则继续优先使用自身响应体，`statusCode` 作为这些 mock 响应的状态码。

## 依赖项

- `crates/bifrost-proxy/src/utils/mock.rs`
- `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs`
- `crates/bifrost-e2e/src/tests/status_redirect.rs`

## 测试方案

### 单元测试

- `test_status_code_with_host_generates_direct_response`：构造 `ResolvedRules { host, status_code }`，验证 `generate_mock_response` 返回指定状态码和响应头。

### E2E 测试

- `status_statusCode_direct_no_upstream`：启动真实 proxy 和 mock upstream，规则为 `host://127.0.0.1:<mock> statusCode://451 resBody://(blocked)`，验证客户端收到 451 + body，且 upstream 请求计数为 0。
- 复跑 `status_replaceStatus_200`，验证 `replaceStatus` 仍请求 upstream 并保留后端 body。

### 真实场景测试

- 创建 `human_tests/status-code-direct-response.md`。
- 使用临时数据目录和 `--no-system-proxy` 启动 Bifrost。
- 通过临时端口绑定 `statusCode + host` 规则，curl 请求后验证返回 451，且本地 mock server 收到 0 个请求。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：`statusCode` 命中后不访问后端。
- 检查 `utils/mock.rs`、`tunnel/mod.rs`、E2E 用例、human_tests 文档。
- 运行最小单元测试与 E2E status 用例。

### 第 2 轮

- 复查 `statusCode` 与 `replaceStatus` 的边界。
- 复查文档和 Web editor 文案是否同步。
- 复跑受影响测试，并执行 `rust-project-validate` 要求的 fmt/clippy/测试。

## 校验要求

- `cargo test -p bifrost-proxy test_status_code_with_host_generates_direct_response`
- `cargo run -p bifrost-e2e -- --test status_statusCode_direct_no_upstream`
- `cargo run -p bifrost-e2e -- --test status_replaceStatus_200`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`

## 文档更新要求

- 更新 `human_tests/readme.md` 索引。
- 更新 Web 编辑器 `statusCode` 协议提示，明确直接返回且不请求 upstream。
