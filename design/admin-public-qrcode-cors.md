# Admin 公开二维码接口 CORS 修复方案

## 背景

Bifrost 管理端在 `/_bifrost/public/*` 前缀下暴露若干“不需要 CSRF、允许跨域被浏览器读取”的公开资源，主要用于分发根证书与移动端配对二维码：

- `GET /_bifrost/public/cert` — 下载 Bifrost 生成的根 CA 证书 `.crt`。
- `GET /_bifrost/public/cert/qrcode` — 生成证书信任向导 URL 的二维码 SVG。
- `GET /_bifrost/public/proxy/qrcode?ip=<lan_ip>` — 生成移动端一键配对代理的二维码 SVG。

这些接口设计上就要被“chat/wiki 页面里的一个 img/iframe”、“Bifrost Web UI 的 fetch”、“iOS/Android 相机扫码后的浏览器 preview”三类调用者跨域访问。历史实现中，`/_bifrost/public/cert` 走了普通 Admin 响应通道，缺少 `Access-Control-Allow-Origin` 且被 absolute-form 检查误判为伪造请求返回 403；`/qrcode` 接口则没有明确处理 `OPTIONS` 预检。本方案统一收敛到公开响应 builder。

## 用户目标验证清单

### 必须实现

- 三条公开路径在任何 `Origin` 下都返回一致、完整的 CORS 响应头集合。
- `OPTIONS` 预检返回 `204` 并携带允许的 methods/headers。
- `Access-Control-Allow-Origin` 严格按调用方 `Origin` 回填，只允许通过 `crates/bifrost-admin/src/cors.rs` `is_allowed_origin` 判定的来源，而不是硬编码 `*`。
- `/_bifrost/public/cert` 不再因为 absolute-form URI（例如 `curl -x` 或 iOS/Android 系统组件构造的代理格式）被拒 `403 Forbidden`。
- `Content-Type` 与实际内容一致（cert 为 `application/x-x509-ca-cert`，qrcode 为 `image/svg+xml`）。

### 必须不破坏

- Admin API `/_bifrost/api/...` 的 CSRF / Origin / absolute-form 拒绝路径不变。
- 非公开路径不能因 builder 复用被意外授予公开 CORS 头。
- 现有 `is_valid_admin_request` 对 absolute-form + Admin API 组合的拒绝语义保持。
- 系统代理默认 bypass 语义不变。

### 必须真实验证

- 用 `curl -H 'Origin: http://allowed.example'` 与 `curl -H 'Origin: http://evil.example'` 分别验证 CORS 头回填/剔除逻辑。
- 用 `curl -X OPTIONS -H 'Origin: ...'` 验证 204 preflight。
- 用 `curl -x http://127.0.0.1:9900 http://127.0.0.1:9900/_bifrost/public/cert` 验证 absolute-form cert 下载 200。
- 用 Bifrost Web UI 在浏览器里加载证书信任向导二维码，DevTools Network 面板确认 CORS 头存在。

## 产品语义

“公开资源”指以下三条严格枚举的路径：

```
GET  /_bifrost/public/cert
HEAD /_bifrost/public/cert
GET  /_bifrost/public/cert/qrcode
GET  /_bifrost/public/proxy/qrcode
OPTIONS /_bifrost/public/**
```

所有其他 `/_bifrost/...` 都不是公开资源，必须走完整的 server + AdminRouter 校验。“Origin 白名单”仍由 `is_allowed_origin` 决定（本机 UI、`bifrost.local` 虚拟 Host、以及未来的自定义 admin domain）；未匹配的 Origin 将不返回 `Access-Control-Allow-Origin`，浏览器 SOP 自然拦截。

CORS 头集合固定：

- `Access-Control-Allow-Methods: GET, HEAD, OPTIONS`
- `Access-Control-Allow-Headers: Content-Type, Authorization, X-Client-Id`
- `Access-Control-Allow-Origin: <matched-origin>` (由 `apply_cors_headers` 回填)

## 技术细节

### 关键常量与函数

- `crates/bifrost-admin/src/handlers/mod.rs`
  - `pub const ADMIN_CORS_ALLOW_HEADERS: &str = "Content-Type, Authorization, X-Client-Id"`
  - `pub const PUBLIC_CORS_ALLOW_METHODS: &str = "GET, HEAD, OPTIONS"`
  - `pub fn public_response_builder(status: StatusCode) -> hyper::http::response::Builder`
- `crates/bifrost-admin/src/cors.rs`
  - `pub fn is_allowed_origin(origin: &str) -> bool`
  - `pub fn allowed_origin_header_value(origin: &str) -> Option<HeaderValue>`
  - `pub fn apply_cors_headers(resp: &mut Response<Body>, origin: Option<&str>)`
- `crates/bifrost-admin/src/security.rs`
  - `pub fn is_cert_public_request<T>(req: &Request<T>) -> bool` — 通过 URI path prefix 判断，忽略 absolute-form scheme/host。
  - `pub fn is_valid_admin_request<T>(req, peer_addr, config, remote_access) -> bool` — 对公开 cert 请求早退，不因 absolute-form 拒绝。
- `crates/bifrost-admin/src/handlers/cert.rs`：cert 下载与二维码 handler。
- `crates/bifrost-admin/src/handlers/mobile_devices.rs`：proxy 二维码 handler。
- `crates/bifrost-admin/src/router.rs`：`AdminRouter::handle` 出口通过 `apply_cors_headers(&mut resp, origin)` 回填 `Access-Control-Allow-Origin`。

### `public_response_builder` 构造

```rust
pub fn public_response_builder(status: StatusCode) -> hyper::http::response::Builder {
    Response::builder()
        .status(status)
        .header("Access-Control-Allow-Methods", PUBLIC_CORS_ALLOW_METHODS)
        .header("Access-Control-Allow-Headers", ADMIN_CORS_ALLOW_HEADERS)
        .header("Vary", "Origin")
    // Access-Control-Allow-Origin 由 apply_cors_headers 在 router 出口注入
}
```

### 公开 cert 路径判定

```rust
pub fn is_cert_public_request<T>(req: &Request<T>) -> bool {
    let path = req.uri().path();
    path == "/_bifrost/public/cert"
        || path == "/_bifrost/public/cert/qrcode"
        || path == "/_bifrost/public/proxy/qrcode"
}
```

在 `is_valid_admin_request` 的最前面：

```rust
if is_cert_public_request(req) {
    return true; // 不做 absolute-form 拒绝，不做 Sec-Fetch-Site 校验
}
```

### OPTIONS 预检

每个公开 handler 在方法分派时显式处理 `OPTIONS`：

```rust
if req.method() == Method::OPTIONS {
    return Ok(public_response_builder(StatusCode::NO_CONTENT)
        .body(Body::empty())
        .unwrap());
}
```

## CLI 交互

无 CLI 变化。CLI 无浏览器上下文，不受 CORS 影响；证书下载沿用 `bifrost cert install/export` 命令，通过内部 API 直接读取。

## Web UI 交互

- 首屏“Trust Certificate”卡片使用 `fetch('/_bifrost/public/cert/qrcode')` 加载二维码。
- 移动端配对页使用 `fetch('/_bifrost/public/proxy/qrcode?ip=<lan_ip>')`。
- 二维码图片可以在 wiki/chat 系统里跨域嵌入（`<img src="http://<lan-ip>:9900/_bifrost/public/cert/qrcode">`），前提是调用方 Origin 通过 `is_allowed_origin`；否则浏览器自身拦截。

## Admin API

无字段变化。响应 header 变化：

| 路径 | 方法 | 变化 |
| --- | --- | --- |
| `/_bifrost/public/cert` | GET/HEAD/OPTIONS | 加 `Access-Control-Allow-*`；absolute-form 不再 403 |
| `/_bifrost/public/cert/qrcode` | GET/OPTIONS | 加 `Access-Control-Allow-*`；OPTIONS 204 |
| `/_bifrost/public/proxy/qrcode` | GET/OPTIONS | 加 `Access-Control-Allow-*`；OPTIONS 204 |

## Sync / 导入导出 / 分享边界

不涉及。CORS 修复不改动 sync/share/import 语义。

## 实现切分

### Phase 1：常量与 builder 落地

- 在 `crates/bifrost-admin/src/handlers/mod.rs` 引入 `ADMIN_CORS_ALLOW_HEADERS`、`PUBLIC_CORS_ALLOW_METHODS`、`public_response_builder`。
- 补充 handler 单元测试 `public_response_builder_includes_cors_headers`。

### Phase 2：三条 handler 迁移

- `handlers/cert.rs` cert download / qrcode 切换到 `public_response_builder`。
- `handlers/mobile_devices.rs` proxy qrcode 切换到同一 builder。
- 每个 handler 显式支持 `OPTIONS`。

### Phase 3：Server 层公开 cert 放行

- `security.rs::is_cert_public_request` 判断按 path prefix，不检查 absolute-form。
- `is_valid_admin_request` 在开头对公开 cert 请求早退。
- 更新单元测试覆盖 absolute-form + public cert 组合。

### Phase 4：Router 出口 CORS 回填

- `AdminRouter::handle` 在返回前调用 `apply_cors_headers(&mut resp, origin_header)`。
- 保证公开 builder + router 回填叠加后不重复写 `Access-Control-Allow-Origin`（builder 只写 `Vary: Origin`）。

### Phase 5：真实回归

- 新增 E2E 脚本、更新 human_tests。

## 测试方案

### 单元测试

- `crates/bifrost-admin/src/handlers/mod.rs::tests::public_response_builder_includes_cors_headers` — 校验三个头都在。
- `crates/bifrost-admin/src/cors.rs::tests::apply_cors_headers_adds_allowed_origin` — 允许 origin 回填。
- `crates/bifrost-admin/src/cors.rs::tests::apply_cors_headers_removes_wildcard_for_disallowed_origin` — 禁止 origin 不注入。
- `crates/bifrost-admin/src/cors.rs::tests::apply_cors_headers_no_origin_header` — 无 Origin 时不注入。
- `crates/bifrost-admin/src/security.rs::tests::test_is_valid_admin_request_allows_absolute_form_for_public_cert` — absolute-form public cert 200。
- `crates/bifrost-admin/src/security.rs::tests::test_is_valid_admin_request_still_rejects_absolute_form_for_admin_api` — Admin API absolute-form 拒绝。

### E2E 测试

- 新增 `e2e-tests/tests/test_admin_public_cors.sh`：
  - `GET /_bifrost/public/proxy/qrcode?ip=127.0.0.1` 200 + `Content-Type: image/svg+xml`。
  - `OPTIONS /_bifrost/public/proxy/qrcode` 204 + `Access-Control-Allow-Methods`。
  - `GET /_bifrost/public/cert/qrcode` 200 + CORS 头。
  - `GET /_bifrost/public/cert` absolute-form 请求 200，返回证书内容。
  - Origin 允许 vs 不允许对比 `Access-Control-Allow-Origin` 值。
- 关联脚本：`e2e-tests/tests/test_admin_cross_site_security.sh` 中的 Admin API absolute-form 拒绝用例不能被本次修改影响。

### 真实场景测试 human_tests

新增 `human_tests/admin-public-qrcode-cors.md`：

- TC-APC-01：Web UI 首屏加载证书二维码，DevTools 检查 CORS 头。
- TC-APC-02：`curl -x http://127.0.0.1:9900 http://127.0.0.1:9900/_bifrost/public/cert` 200。
- TC-APC-03：`curl -X OPTIONS -H 'Origin: http://localhost:8800' http://127.0.0.1:9900/_bifrost/public/proxy/qrcode` 204。
- TC-APC-04：`curl -H 'Origin: http://evil.example' ...` 响应不包含 `Access-Control-Allow-Origin`。
- TC-APC-05：iOS/Android 相机扫描证书二维码，链接可打开。

同时在 `human_tests/readme.md` 索引里追加条目。

### 覆盖率与项目校验

- `cargo test -p bifrost-admin cors::`
- `cargo test -p bifrost-admin handlers::`
- `cargo test -p bifrost-admin security::`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `rust-project-validate`

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核三条 public handler 是否都换成 `public_response_builder`。
- 复核 `is_valid_admin_request` 早退分支是否严格限制在三条 public path。
- 复核 `apply_cors_headers` 出口是否与 builder 冲突写头。
- 复测 handler 单测、CORS 单测、E2E 脚本。

### 第 2 轮

- 复查 `human_tests/admin-public-qrcode-cors.md` 与 `human_tests/readme.md` 索引。
- 复跑 `e2e-tests/tests/test_admin_public_cors.sh` 与 `test_admin_cross_site_security.sh`，保证 CORS 改动没有绕过 Admin API 的拒绝路径。
- 全量 `cargo test --workspace --all-features`。

## 风险与决策点

- **不硬编码 `*`**：即便是“公开”资源，也仍受 `is_allowed_origin` 白名单约束。这样如果未来公司安全策略要求“只允许 `https://bifrost.corp` 加载证书二维码”，只需扩 `cors.rs` 白名单即可。
- **absolute-form public cert 早退**：早退只覆盖三条 path，不能扩散到 `/_bifrost/api/`。security 单测必须两侧都覆盖，防止回归把 Admin API 也放行。
- **Content-Type**：cert 与 qrcode 类型不同，builder 只统一 header 集，具体 `Content-Type` 仍由各 handler 设置。
- **文档更新范围**：本次不涉及外部 API 语义变化，`README.md`、`crates/bifrost-admin/ADMIN_API.md` 无需变动。仅新增 `human_tests/admin-public-qrcode-cors.md` 并更新索引。
- **iOS/Android 系统组件构造的代理格式**：真实设备扫码后可能发出 absolute-form 请求。这也是这次要放开 absolute-form public cert 的直接动因；后续如引入新的公开路径要一并进入 `is_cert_public_request` 白名单。
