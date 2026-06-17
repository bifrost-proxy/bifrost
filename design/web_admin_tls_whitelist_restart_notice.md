# 管理端 TLS 白名单变更后的重连提示

## 功能模块详细描述

- 在管理端操作 TLS 白名单后，成功提示除了展示“已添加/已移除”结果，还要明确提醒用户重启目标应用并重新打开目标域名。
- 当前覆盖入口（已实现 2026-06-17）：
  - `Settings -> Proxy -> TLS Interception Patterns`（域名/应用白名单的增删）。
  - `Network -> 选择 CONNECT 请求 -> 详情 TunnelInterceptActions` 中的 `Intercept this domain` / `Intercept this app` / `Intercept this client` 按钮。
  - `Network -> 流量表格右键 TrafficContextMenu` 中相同的三类快捷加入操作。
- 当前 TLS 白名单实际为三类：域名白名单（`intercept_include`）、应用白名单（`app_intercept_include`）、客户端 IP 白名单（`ip_intercept_include`）。原设计中只提到前两类；IP 白名单是同期落地的扩展。

## 实现逻辑

- 在 `web/src/utils/tlsInterceptionNotice.ts` 抽取统一的成功提示方法。
- 当新增或删除域名白名单、应用白名单成功后，统一追加重连提醒文案：
  `Restart the target app and reopen the target domain to establish a new connection.`
- `TrafficDetail/TunnelInterceptActions` 和 `TrafficTable/TrafficContextMenu` 中的域名 / 应用 / 客户端 IP 加白操作成功后，都复用同一提示。
- 保持失败提示与其他 TLS 设置项一致，不改变接口调用和状态更新逻辑。

## 依赖项

- 复用前端现有 `antd` 的 `message.success` 提示能力。
- 复用现有 TLS 配置更新接口 `updateTlsConfig`。

## 测试方案（含 e2e）

- `web/tests/ui/admin-settings.spec.ts` 已断言 Settings 入口新增 TLS 域名白名单后提示包含 `Restart the target app and reopen the target domain to establish a new connection.`。
- `web/tests/ui/traffic-push.spec.ts` 已覆盖 `Intercept this app` / `Intercept this client` 按钮触发重连提醒，并校验配置接口写入 `app_intercept_include` / `ip_intercept_include`。
- （planned, not yet shipped as of 2026-06-17）尚未对 `Intercept this domain` 按钮以及 `TrafficContextMenu` 三类操作单独补充重连提醒断言。
- 按项目要求先执行相关 E2E，再执行 `rust-project-validate`。

## 校验要求（含 rust-project-validate）

- 执行与管理端设置页相关的 UI E2E，确认提示展示正确且不影响原有保存逻辑。
- 在 E2E 完成后执行 `cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features -- -D warnings`、按改动范围执行测试与构建。

## 文档更新要求

- 当前变更仅涉及交互提示与测试说明，无需更新 `README.md`。
- 若后续把同类提示扩展到 TLS 黑名单或其他配置项，应同步补充到管理端 UI E2E 说明。
