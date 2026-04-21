# 管理端 Push WebSocket 代理访问修复

## 功能模块描述

修复通过 Bifrost 自身 HTTP 代理端口访问管理端时，前端 `/_bifrost/api/push` WebSocket 连接被错误当作普通上游 WebSocket 转发的问题，避免管理端实时推送通道反复断开重连。

## 实现逻辑

- 在 `crates/bifrost-proxy/src/proxy/http/handler.rs` 的 `handle_http_websocket` 中增加“本地管理端 WebSocket”识别逻辑。
- 当满足以下条件时，直接交给 `AdminRouter::handle`，不再通过 HTTP 代理链路回拨自身端口：
  - 目标 Host 为 `bifrost.local`；或
  - 请求路径位于 `/_bifrost/...` 下，且目标端口等于当前监听端口，Host 为 `localhost`、`127.0.0.1` 或 `::1`
- 对 `bifrost.local` 虚拟 Host 的管理端请求，沿用已有路径补写规则，将 `/api/push` 重写为 `/_bifrost/api/push` 后再进入管理端路由。
- 为本地管理端短路分支补充单元测试，确保 Host/端口/路径判定稳定。

## 依赖项

- `bifrost_admin::AdminRouter`
- `bifrost_admin::SharedPushManager`
- 现有管理端 Push WebSocket 处理器

## 测试方案

- 单元测试：
  - `test_should_route_websocket_to_local_admin_for_loopback_admin_path`
  - `test_should_not_route_websocket_to_local_admin_for_non_admin_path_or_port`
  - `test_rewrite_local_admin_websocket_request_rewrites_virtual_host_path`
- E2E 测试：
  - 在 `e2e-tests/tests/test_traffic_push_e2e.sh` 新增“通过 HTTP 代理访问管理端 Push WebSocket”回归用例，断言收到 `connected` 和 `overview_update`
- 真实场景测试：
  - 更新 `human_tests/api-push.md`，新增“通过代理访问管理端 Push WebSocket”回归用例
  - 同步更新 `human_tests/readme.md`

## 校验要求

- `cargo test -p bifrost-proxy --all-features`
- 目标 E2E 脚本通过
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`

## 文档更新要求

- 更新 `human_tests/api-push.md`
- 更新 `human_tests/readme.md`
