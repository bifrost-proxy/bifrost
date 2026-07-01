# 管理端虚拟 Host

## 功能模块说明

`bifrost.local` 是 Bifrost 管理端的虚拟 Host。用户通过浏览器或显式代理访问 `http://bifrost.local/`、`https://bifrost.local/` 时，预期等价于访问当前 Bifrost 实例的管理端首页，而不是被当作普通外部域名解析或转发。

## 实现逻辑

- 请求进入代理服务后，先判断是否为非 `CONNECT` 的 `bifrost.local` 管理虚拟 Host 请求。
- 若命中管理虚拟 Host，则在“是否代理到其他服务器”的判定中优先视为当前 Bifrost 实例自身，不再因为 absolute-form URI 默认端口为 `80` 而误判为外部目标。
- 管理虚拟 Host 请求仍保留本地访问限制：只有 loopback 来源可以直接打开管理端；远端来源继续返回 `403`。
- `CONNECT` 请求不会被识别为管理虚拟 Host，避免 HTTPS 隧道或外部代理语义被误路由到管理 UI。
- HTTPS 访问通过现有 CONNECT tunnel 路径处理：本地客户端访问 `bifrost.local:443` 时强制启用管理虚拟 Host 的 TLS intercept，解包后的请求由 Admin Router 响应。
- 普通 absolute-form 外部请求继续按代理目标转发，不受 `bifrost.local` 特例影响。
- 系统代理和 CLI proxy 的默认 bypass 只包含 loopback，不再包含 `*.local`。否则浏览器或 shell 会绕过 Bifrost 直接解析 `bifrost.local`，导致 DNS 错误。用户显式传入包含 `*.local` 的 bypass 时仍按用户配置执行。

## 依赖项

- `crates/bifrost-proxy/src/server.rs` 的请求路由判断。
- Admin Router 的现有 `rewrite_virtual_host_request` 路径。
- CONNECT tunnel 中已有的 `bifrost.local` TLS intercept 和 `rewrite_intercepted_virtual_host_request` 路径。
- 现有 WebUI 静态资源和 Admin API。

## 测试方案

### 单元测试

- `test_admin_virtual_host_absolute_uri_without_port_routes_to_admin`：验证 `GET http://bifrost.local/` 虽然按通用代理目标判断会是外部目标，但最终路由判定会视为管理端自身。
- `test_admin_virtual_host_absolute_uri_with_self_port_routes_to_admin`：验证带当前代理端口的历史路径仍然进入管理端。
- `test_external_absolute_uri_still_routes_to_proxy_target`：验证普通外部 absolute-form 请求仍然按代理目标转发。
- `test_connect_to_admin_virtual_host_is_not_admin_ui_request`：验证 `CONNECT` 不进入管理虚拟 Host。
- storage 默认配置单测验证系统代理默认 bypass 不再包含 `*.local`。

### E2E 测试

- `e2e-tests/tests/test_admin_virtual_host_proxy.sh` 启动真实临时 Bifrost 服务和本地 HTTP target。
- 通过 `curl -x http://127.0.0.1:<port> http://bifrost.local/` 验证管理端 HTML 返回。
- 通过 `curl -k -x http://127.0.0.1:<port> https://bifrost.local/` 验证 HTTPS 管理端虚拟 Host 返回。
- 验证 `http://bifrost.local:<port>/` 与 `Host: bifrost.local` 直连路径保持可用。
- 验证 Admin API 暴露的默认系统代理 bypass 不包含 `*.local`。
- 验证普通 `http://127.0.0.1:<target_port>/ordinary-target` 仍通过代理转发到外部 target。

### 真实场景测试

- `human_tests/admin-virtual-host.md` 记录并执行管理虚拟 Host 的真实代理访问回归。
- 用例覆盖 HTTP/HTTPS 不带端口的 `bifrost.local`、带端口的旧路径、Host header 直连、默认 bypass 和普通代理目标边界。

## Review/Fix/Test 闭环方案

- 第 1 轮：复核用户目标、`server.rs` 路由顺序、单元测试和 E2E 脚本，运行受影响单元测试与 E2E。
- 第 2 轮：复核第 1 轮修复后的 diff、human_tests 索引与真实执行记录，复跑受影响测试并确认普通代理目标边界没有回归。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo test -p bifrost-proxy admin_virtual_host -- --nocapture`
- `BIFROST_DISABLE_TRAY=1 BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_admin_virtual_host_proxy.sh`
- `cargo test --workspace --all-features`
- `make coverage`，如完整 E2E 覆盖环境不可用则退化为 `make coverage-unit` 并说明原因。
- 收尾阶段执行 rust-project-validate 技能要求的校验。

## 文档更新要求

- 更新 `human_tests/admin-virtual-host.md`。
- 更新 `human_tests/readme.md` 索引。
- 本次不新增 CLI 参数、协议字段或 WebUI 文案，不需要更新 README。
