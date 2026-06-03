# System Proxy Ownership

## 功能模块说明

Bifrost 的 System Proxy 只应管理自己写入的系统代理配置。用户同时运行 Surge、Clash、系统级 VPN/代理等外部代理时，Bifrost 可以展示真实系统代理状态，但不能把外部代理误判为自身代理，也不能在 `system-proxy disable`、Admin UI 关闭开关或 `bifrost stop` 时清除外部代理。

## 实现逻辑

- `SystemProxyManager::enable` 在写入系统代理前保存两类状态：`proxy_backup.json` 继续兼容旧恢复逻辑；新增 `proxy_state.json` 记录 Bifrost 本次写入的 target 与写入前 original。
- 关闭路径新增归属判定：只有当前系统代理 host/port 与 Bifrost target 匹配时，才恢复 original 或关闭；如果当前代理指向其他端口或其他 host，则返回 `OwnedByOther` 并保持系统设置不变。
- `bifrost stop` 不再在缺失 runtime 时把任意本机端口代理当作 Bifrost 代理；必须有 runtime host/port 且与当前系统代理匹配才会清理。
- Admin API `GET /api/proxy/system` 返回 `managed_by_bifrost`，WebUI disable 验证在外部代理仍开启但 `managed_by_bifrost=false` 时视为成功，避免误报 `System proxy is still enabled`。

## 依赖项

- macOS 使用 `networksetup` / `scutil --proxy` 获取和写入系统代理。
- Windows/Linux 继续复用现有 `sysproxy` / 注册表 / gsettings 路径。
- WebUI 复用 `SystemProxyStatus`，新增字段保持 optional，兼容旧服务端响应。

## 测试方案

- 单元测试：
  - `ProxyBackup::target_matches` 覆盖 loopback host alias 和端口不匹配。
  - Admin disable 验证覆盖“外部代理仍启用但不归 Bifrost 管理时视为关闭成功”。
  - CLI stop host 判定覆盖 wildcard listen host 到 loopback 的映射。
- E2E 测试：更新 `e2e-tests/tests/test_system_proxy_e2e.sh`，新增 macOS 外部代理回归：先设置外部本机端口代理，启动 Bifrost `--no-system-proxy`，调用 Admin API disable，断言外部代理仍保留且返回 `managed_by_bifrost=false`。
- 真实场景测试：更新并执行 `human_tests/cli-system-proxy.md` 的 Surge/外部代理回归用例，验证 CLI disable 与 stop 不清理外部代理。

## Review/Fix/Test 闭环方案

- 第 1 轮：复核用户问题、`git diff`、SystemProxyManager/CLI stop/Admin API/Web store 改动，运行核心单测和系统代理 E2E。
- 第 2 轮：复查第 1 轮修复后 diff、design/human_tests/readme 一致性，复跑受影响单测、E2E 和 human_tests。

## 校验要求

- 必须运行 `cargo test -p bifrost-core system_proxy`、`cargo test -p bifrost-admin proxy::tests`、`cargo test -p bifrost-cli commands::stop::tests`。
- 必须运行 `bash e2e-tests/tests/test_system_proxy_e2e.sh`（macOS/Windows 支持平台；使用临时 `BIFROST_DATA_DIR`）。
- 收尾前按仓库规则运行 `cargo test --workspace --all-features` 与 `rust-project-validate`。

## 文档更新要求

- 更新 `human_tests/cli-system-proxy.md` 增加外部代理归属回归。
- 更新 `human_tests/readme.md` CLI 系统代理用例数与说明。
