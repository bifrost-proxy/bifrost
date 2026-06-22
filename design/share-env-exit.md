# Share 环境快速退出

## 功能模块说明

Share rule query 会把分享规则导入 My Rules，并以 exclusive 方式启用该规则、禁用其它 My Rules。该能力适合快速进入预览环境，但用户需要一个明确的 Share 环境提示和快速退出入口，退出后恢复进入 Share 前的启用规则集合。

## 实现逻辑

- `RulesStorage` 在 rules 目录下维护 `.share_env_state.json`，记录 `active`、导入的规则名、原始分享名、内容 hash、进入前启用的 My Rules 列表和进入时间。
- `import_rule_share_payload` 在第一次进入 Share 环境时保存当前 enabled My Rules 快照；如果已经处于 active Share 环境，继续导入其它 share 链接时不覆盖原始快照。
- Share 导入仍保持原有行为：目标分享规则启用，其它 My Rules 禁用，Group 规则不参与第一版。
- `POST /_bifrost/api/rules/share-env/exit` 按快照恢复 My Rules enabled 状态，清除 `.share_env_state.json`，并复用 rules changed 通知链路刷新 resolver 和 badge cache。
- `GET /_bifrost/api/rules/share-env/status` 返回当前 Share 环境状态，便于 UI、E2E 和真实场景验证。
- 注入页面的 Bifrost badge JSON 增加 `share_env` 字段；当 `share_env.active=true` 时，B 胶囊立即进入 Share 视觉状态：边框呼吸光晕、右上角红点，hover panel 中展示短文案 `Share preview active` 和 `Exit` 按钮。
- Exit 按钮优先以 CORS POST 调用本地 admin API；业务页面跨域限制导致 CORS 请求失败时，降级为 `no-cors` POST 触发退出，成功后刷新当前页面，让后续请求使用恢复后的规则状态。

## 依赖项

- `bifrost-storage::RulesStorage`
- `bifrost-admin::rule_share_import`
- `bifrost-proxy::transform::badge`
- 现有 Admin CORS 与 rules changed 通知链路

## 测试方案

### 单元测试

- `bifrost-storage`：`test_share_env_state_roundtrip_and_clear` 验证 Share 状态文件保存、读取和清理。
- `bifrost-admin`：`import_stashes_pre_share_enabled_rules_once_and_exit_restores` 验证第一次 Share 导入保存快照、第二次 Share 不覆盖快照、退出恢复原 enabled 集合。
- `bifrost-admin`：`exit_share_env_without_state_is_noop` 验证无 Share 状态时退出是幂等 no-op。
- `bifrost-proxy`：`test_badge_share_env_badge_and_exit_button_present` 验证注入片段包含 Share 红点、呼吸光晕、初始化 Share 状态、`Share preview active` 行、Exit 按钮和退出 API 的 `no-cors` 兜底。

### E2E 测试

- `e2e-tests/tests/test_badge_injection_e2e.sh`
  - 创建 `before-enabled`、`before-disabled`、`share-source` 三条 My Rules。
  - 通过真实代理访问包含 `__bifrost_rule` 的分享 URL，断言 GET 重定向到 clean URL。
  - 断言 Share 状态 API active，注入页面包含 Share 红点、呼吸光晕、`Share preview active` 和退出 API。
  - 调用退出 API 后断言 `before-enabled` 恢复 enabled，`before-disabled` 保持 disabled，导入的 `share/share-source` 被 disabled。

### 真实场景测试

- 新增 `human_tests/share-env-exit.md`。
- 使用临时数据目录启动 Bifrost：`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 BIFROST_DATA_DIR=... bifrost start -p <port> --unsafe-ssl --skip-cert-check --no-system-proxy --enable-badge-injection`。
- 通过真实代理访问分享链接，确认页面胶囊出现呼吸光晕和右上角红点，hover panel 显示 `Share preview active` 和 `Exit`。
- 点击或调用 Exit，确认规则状态恢复。

## Review/Fix/Test 闭环方案

- 第 1 轮：复核 share 导入、状态持久化、badge JSON 和退出 API diff，运行 storage/admin/proxy 窄单测与 E2E share case。
- 第 2 轮：复查第 1 轮后的 diff、human_tests 索引与真实场景步骤，复跑相关单测、E2E 和 human_tests。

## 校验要求

- `cargo test -p bifrost-storage share_env -- --nocapture`
- `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin rule_share -- --nocapture`
- `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-proxy badge_share_env -- --nocapture`
- `bash e2e-tests/tests/test_badge_injection_e2e.sh`
- `cargo test --workspace --all-features`
- `make coverage`
- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`

## 文档更新要求

本次不新增 CLI 参数、README 协议列表或 Hook；需要更新 `human_tests/readme.md`，新增 Share 环境退出真实场景索引。
