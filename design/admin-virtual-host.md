# 管理端虚拟 Host `bifrost.local`

## 背景

Bifrost 管理端历史上要求用户访问 `http://127.0.0.1:9900/`（或当前监听端口）打开 Web UI。这带来几个不方便：

- 端口非稳定（用户可能改成非默认端口，或短期跑 temporary port），书签容易失效。
- 手机、平板浏览器通过 LAN 访问 Bifrost 时要背 IP + 端口。
- 通过 Bifrost 代理访问 Web UI 时，`http://<lan-ip>:9900/` 可能被 proxy 逻辑误判为“上游目标”。

`bifrost.local` 是 Bifrost 管理端的虚拟 Host。用户在浏览器或已经设置了 Bifrost 系统代理的场景下，访问 `http://bifrost.local/`、`https://bifrost.local/` 时，都应等价于访问“当前 Bifrost 实例的管理端首页”。

这个虚拟 Host 不需要用户在本机配置 `/etc/hosts`；实现依赖“任何走进 Bifrost 代理的、Host = `bifrost.local` 的请求都由 Admin Router 响应”。

## 用户目标验证清单

### 必须实现

- 浏览器把系统代理指向 Bifrost 后，`http://bifrost.local/` 打开 Admin Web UI（登录页 / 首屏 / 静态资源均可用）。
- `https://bifrost.local/` 通过 CONNECT tunnel 进入 Bifrost，MITM 解包后返回 Admin UI。
- 老路径 `http://bifrost.local:<listen_port>/` 与 `Host: bifrost.local` 直连兼容。
- 只有 loopback 来源可以直接打开管理端；远端来源继续按 remote access 配置返回 403 或允许。
- `CONNECT` 请求不会被误识别为 admin virtual host，避免 HTTPS 隧道或代理链被路由到 Admin UI。
- 系统代理默认 bypass 不再包含 `*.local`，否则浏览器/Shell 会绕过 Bifrost 直接解析 `bifrost.local`，触发 DNS 错误。
- 用户显式传入包含 `*.local` 的 bypass 时按用户配置执行，不擅自剔除。

### 必须不破坏

- 普通 absolute-form 外部请求继续按代理目标转发，不受 `bifrost.local` 特例影响。
- 主端口正常 Admin API `/_bifrost/api/...` 请求不变。
- CONNECT + 上游 TLS intercept 语义不变。
- Rule Share 确认页、Admin WebSocket、CORS 等既有逻辑保持。
- Mobile 端 `bifrost.local` 场景下依赖 CA 已信任；未信任时按现有证书信任向导展示。

### 必须真实验证

- 用真实 Bifrost + curl 验证 `http://bifrost.local/`、`https://bifrost.local/`、`http://bifrost.local:<port>/` 三种入口。
- 通过 `Host: bifrost.local` header 直连（不走代理）仍可访问。
- 普通代理目标 `http://127.0.0.1:<target_port>/ordinary-target` 仍正常转发到外部 target。
- Admin API 暴露的默认系统代理 bypass 不再包含 `*.local`。

## 产品语义

### `bifrost.local` 的三种入口

| 入口 | 场景 | 处理方式 |
| --- | --- | --- |
| `http://bifrost.local/` via Bifrost proxy | 浏览器已把系统代理指向 Bifrost | 代理层识别 admin virtual host，转 Admin Router |
| `https://bifrost.local/` via Bifrost proxy | 浏览器 HTTPS 首访 | CONNECT tunnel 到 `bifrost.local:443`，强制 TLS intercept，解包后转 Admin Router |
| `http://bifrost.local:<port>/` 直连 or `Host: bifrost.local` | 用户直接连本机端口 | server 层识别虚拟 Host，`rewrite_virtual_host_request` 后转 Admin Router |

其中前两个入口在“是否代理到其他服务器”的判定中不能被 absolute-form URI 的默认端口 80/443 误判为外部目标。

### 与 `is_valid_admin_request` 的关系

Admin virtual host 请求也要过 `is_valid_admin_request`；Host = `bifrost.local` 属于白名单，绕过 DNS rebinding 拒绝。但 remote peer + Admin API 组合仍按现有 remote access 策略处理。

### 系统代理默认 bypass

macOS/Windows/Linux 上，Bifrost 提供“启用系统代理”能力。默认 bypass 列表来自 `crates/bifrost-storage/src/config_manager.rs`；本方案要求默认 bypass 只包含 loopback (`127.0.0.1`, `localhost`)，不能包含 `*.local`。否则浏览器把 `bifrost.local` 视为 bypass 域名，绕过 Bifrost 直接向操作系统解析，会得到 DNS 失败。

## 技术细节

### 关键常量与函数

- `crates/bifrost-proxy/src/server.rs`
  - `pub(crate) const ADMIN_VIRTUAL_HOST: &str = "bifrost.local";`
  - `fn is_admin_virtual_host_request<B>(req: &Request<B>) -> bool`
  - `fn rewrite_virtual_host_request(req: Request<Incoming>) -> Request<Incoming>`
  - server 路由主循环中 `is_proxy_request_to_other_server` 判定 + admin virtual host 短路（约 `server.rs:1517` `is_admin_virtual_host = is_admin_virtual_host_request(&req)`，`server.rs:1604` `if is_admin_virtual_host && !is_proxy_request_to_other_server`）。
- `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs`
  - `rewrite_intercepted_virtual_host_request` — CONNECT tunnel 内 TLS intercept 后的路径重写。
  - `bifrost.local` TLS intercept 触发条件与常规规则解耦，强制启用。
- `crates/bifrost-admin/src/lib.rs`
  - `AdminRouter::handle` 处理 Admin 请求。
- `crates/bifrost-storage/src/config_manager.rs`
  - 默认系统代理 bypass 列表定义。

### `is_admin_virtual_host_request` 判定

```rust
fn is_admin_virtual_host_request<B>(req: &Request<B>) -> bool {
    if req.method() == Method::CONNECT {
        return false;
    }
    let host = req
        .headers()
        .get(hyper::header::HOST)
        .and_then(|v| v.to_str().ok())
        .or_else(|| req.uri().host())
        .map(|h| h.trim_end_matches(|c: char| c == ':' || c.is_ascii_digit()))
        .unwrap_or("");
    host.eq_ignore_ascii_case(ADMIN_VIRTUAL_HOST)
}
```

### 判定顺序

```rust
let is_admin_virtual_host = is_admin_virtual_host_request(&req);
let is_proxy_request_to_other_server = detect_proxy_target(&req, ctx.self_port);

if is_admin_virtual_host && !is_proxy_request_to_other_server {
    let req = rewrite_virtual_host_request(req);
    return AdminRouter::handle(req, admin_state, push_manager).await;
}
```

其中 `is_proxy_request_to_other_server` 排除 admin virtual host 后，才让 absolute-form URI 走普通代理转发。

### HTTPS 场景

CONNECT `bifrost.local:443` 命中 tunnel handler，无论 rule 是否命中都强制启用 TLS intercept；解包后请求进入 `rewrite_intercepted_virtual_host_request`，再交给 AdminRouter。这要求本机 Bifrost CA 已被信任，否则浏览器会报证书错误——文档需在“HTTPS 场景”一节明确指引用户先信任 CA。

## CLI 交互

无新增 CLI 参数。相关命令：

- `bifrost start`：启动 Bifrost；`bifrost.local` 虚拟 Host 默认可用。
- `bifrost proxy system enable`：开启系统代理；默认 bypass 由 storage 决定，不再包含 `*.local`。
- `bifrost proxy system status`：显示当前 bypass 列表，用户可直观确认。

## Web UI 交互

- 首屏“打开 Admin Web UI”卡片建议链接：`http://bifrost.local/`（仅在系统代理已启用时可用）。
- 证书信任向导页面告知：“如果 `bifrost.local` 无法访问，请先启用 Bifrost 系统代理，或访问 `http://127.0.0.1:<port>/`。”
- Mobile pairing 二维码内嵌 `http://bifrost.local/` 备选入口，方便手机安装 CA 后直接访问 Admin。

## Admin API

- 无新增字段。
- `GET /_bifrost/api/config/system-proxy`：返回默认 bypass 列表，前端展示为可编辑列表，默认不包含 `*.local`。

## Sync / 导入导出 / 分享边界

不涉及。

## 实现切分

### Phase 1：Server 层判定

- 引入 `ADMIN_VIRTUAL_HOST` 常量。
- 实现 `is_admin_virtual_host_request`。
- 调整 `is_proxy_request_to_other_server` 与主路由分支的判定顺序。
- 单元测试覆盖 absolute-form 无端口、绑当前端口、外部 host、CONNECT 四种情况。

### Phase 2：CONNECT tunnel 强制 intercept

- `bifrost.local:443` CONNECT 强制走 TLS intercept。
- `rewrite_intercepted_virtual_host_request` 完成路径与 host 头修正。
- 单测覆盖 tunnel 内路径重写。

### Phase 3：系统代理默认 bypass 清理

- `config_manager.rs` 默认 bypass 移除 `*.local`。
- 保留用户自定义 bypass 的透传逻辑。
- 单测：默认 bypass 不含 `*.local`；用户显式包含时被保留。

### Phase 4：E2E + human_tests

- 新增 `e2e-tests/tests/test_admin_virtual_host_proxy.sh`。
- 新增 `human_tests/admin-virtual-host.md` 与 `human_tests/readme.md` 索引。

## 测试方案

### 单元测试

- `test_admin_virtual_host_absolute_uri_without_port_routes_to_admin`：`GET http://bifrost.local/` absolute-form 判定为 admin。
- `test_admin_virtual_host_absolute_uri_with_self_port_routes_to_admin`：`http://bifrost.local:9900/_bifrost/` 判定为 admin。
- `test_admin_virtual_host_host_header_only_routes_to_admin`：`Host: bifrost.local` 直连判定为 admin。
- `test_external_absolute_uri_still_routes_to_proxy_target`：`GET http://external.example/` 仍走代理转发。
- `test_connect_to_admin_virtual_host_is_not_admin_ui_request`：`CONNECT bifrost.local:443` 不进入 admin 分支。
- `test_admin_virtual_host_case_insensitive`：`BIFROST.LOCAL` 大小写不敏感命中。
- storage `test_default_system_proxy_bypass_excludes_dot_local`：默认 bypass 不含 `*.local`。
- storage `test_user_supplied_bypass_preserves_dot_local`：用户传入 `*.local` 被保留。

### E2E 测试

- 新增 `e2e-tests/tests/test_admin_virtual_host_proxy.sh`：
  - 启动临时数据目录 + 非默认端口 Bifrost。
  - `curl -x http://127.0.0.1:<port> http://bifrost.local/` 断言 HTML 返回。
  - `curl -k -x http://127.0.0.1:<port> https://bifrost.local/` 断言 HTTPS admin UI 返回。
  - `curl --resolve bifrost.local:<port>:127.0.0.1 http://bifrost.local:<port>/_bifrost/api/csrf` 断言 200。
  - `curl -x http://127.0.0.1:<port> http://external.example/` 断言正常走代理转发路径。
  - `curl http://127.0.0.1:<port>/_bifrost/api/config/system-proxy` 断言 bypass 不含 `*.local`。

### 真实场景测试 human_tests

- `human_tests/admin-virtual-host.md` 已存在，用例覆盖：
  - TC-AVH-01：`http://bifrost.local/` 在启用系统代理后打开 Admin UI。
  - TC-AVH-02：`https://bifrost.local/` 在已信任 CA 后打开 Admin UI。
  - TC-AVH-03：`http://bifrost.local:<port>/` 老路径兼容。
  - TC-AVH-04：`Host: bifrost.local` 直连兼容。
  - TC-AVH-05：默认 bypass 不含 `*.local`。
  - TC-AVH-06：普通代理目标 `http://<lan-ip>:<port>/ordinary-target` 仍走转发。
  - TC-AVH-07：远端 peer 访问 `bifrost.local` 默认 403，开启 remote access 后按配置。
- 更新 `human_tests/readme.md` 索引。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo test -p bifrost-proxy admin_virtual_host -- --nocapture`
- `cargo test -p bifrost-storage default_system_proxy_bypass`
- `BIFROST_DISABLE_TRAY=1 BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_admin_virtual_host_proxy.sh`
- `cargo test --workspace --all-features`
- `rust-project-validate`
- 若完整 E2E 覆盖环境不可用，退化为 `make coverage-unit` 并说明原因。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：三种入口都能命中 admin。
- 复核 `server.rs` 路由顺序（先判 admin virtual host，再判 proxy target），避免逆序。
- 复核 tunnel `rewrite_intercepted_virtual_host_request` 是否正确处理 HTTPS。
- 复核 storage 默认 bypass 单测。
- 复测受影响单元测试与 E2E。

### 第 2 轮

- 复核第 1 轮修复后的 diff、`human_tests/admin-virtual-host.md` 与真实执行记录。
- 复跑受影响测试并确认普通代理目标边界没有回归。
- 复查 admin cross-site security 未被虚拟 Host 短路绕过。

## 风险与决策点

- **`bifrost.local` 与其它服务冲突**：企业内部有些开发工具已经把 `*.local` 挂到内网 DNS。默认 bypass 移除 `*.local` 意味着这类域名会走 Bifrost 代理。若用户不希望这样，可以在 `bifrost proxy system` 里显式加回。文档需明确该权衡。
- **HTTPS 首次访问信任问题**：`https://bifrost.local/` 首次访问时 Bifrost CA 未信任会报证书错误。方案不做静默注入 CA，只在文档指引先走 `http://bifrost.local/` 或系统代理 setup 页安装 CA。
- **CONNECT 判定歧义**：CONNECT 请求带 `bifrost.local:443` 目的地，但不进入 admin UI 分支；tunnel 层再单独识别 `bifrost.local` 强制 intercept。两处判定必须独立且互不依赖，避免同一请求两次判定不一致。
- **Absolute-form 默认端口**：`GET http://bifrost.local/` absolute-form 的默认端口是 80，若把这类请求当外部目标转发就会尝试连 80 上的“真实 bifrost.local”，必然失败。方案通过 `is_admin_virtual_host && !is_proxy_request_to_other_server` 早退避免。
- **本次不新增外部协议字段/CLI/WebUI 文案**，不需要更新 `README.md`；仅补 `human_tests/admin-virtual-host.md` 与索引。
