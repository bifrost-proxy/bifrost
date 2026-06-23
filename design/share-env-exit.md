# Share 环境快速退出

## 功能模块说明

Rule Share 确认页应用分享规则后，会把分享规则导入 My Rules，并以 exclusive 方式启用该规则、禁用其它 My Rules。该能力适合快速进入预览环境，但用户需要一个明确的 Share 环境提示和快速退出入口，退出后恢复进入 Share 前的启用规则集合。

## 实现逻辑

- `RulesStorage` 在 rules 目录下维护 `.share_env_state.json`，记录 `active`、导入的规则名、原始分享名、内容 hash、进入前启用的 My Rules 列表、进入时间和 share exit token。
- `share-confirm` 在用户确认应用分享规则后调用 `import_rule_share_payload`。第一次进入 Share 环境时保存当前 enabled My Rules 快照；如果已经处于 active Share 环境，继续确认其它 share 链接时不覆盖原始快照。
- Share 导入仍保持原有行为：目标分享规则启用，其它 My Rules 禁用，Group 规则不参与第一版。
- `POST /_bifrost/api/rules/share-env/exit` 按快照恢复 My Rules enabled 状态，清除 `.share_env_state.json`，并复用 rules changed 通知链路刷新 resolver 和 badge cache。
- `GET /_bifrost/api/rules/share-env/status` 返回当前 Share 环境状态，便于 UI、E2E 和真实场景验证。
- 注入页面的 Bifrost badge JSON 增加 `share_env` 字段；当 `share_env.active=true` 时，B 胶囊立即进入 Share 视觉状态：边框呼吸光晕、右上角红点，hover panel 中展示短文案 `Share preview active` 和 `Exit` 按钮。注入数据包含退出用 `exit_token`，但不包含恢复快照 `enabled_rule_names`。
- Exit 按钮仅在真实用户点击事件中触发，在当前页面直接以 JSON body POST 到 `/_bifrost/api/rules/share-env/exit`；后端校验 token 且响应成功后，当前页面调用 `location.reload()` 刷新，不再打开二级确认页。
- 本地确认页 `/_bifrost/share-env/exit` 仍保留为兼容入口，支持 JSON body 或 form body POST 到 `/_bifrost/api/rules/share-env/exit`。token 不通过 URL query 传递，确认页响应设置 `Cache-Control: no-store`、`Referrer-Policy: no-referrer` 和 `X-Frame-Options: DENY`。
- `/_bifrost/share-env/exit` 确认页不应用全局 CORS，即使请求来自全局允许的本地 origin（如 `http://localhost:3000`），也不能通过浏览器读取 token-bearing HTML。
- 远程访问开启时，确认页复用 `/api/rules/share-env/exit` 的 API 鉴权契约：本机 loopback 访问仍可用，远端访问必须具备有效 Admin JWT，避免远端未授权读取确认页 token。
- `/api/rules/share-env/exit` 提供注入页面专属 CORS 响应，允许业务页面在携带正确 JSON body token 时读取成功响应并刷新当前页面；无 token 或 token 不匹配仍返回 403。
- 如果旧状态文件中 `exit_token` 为空，确认页会重新生成并持久化 token；直接调用 Exit API 遇到空 token 会返回 409，提示重新打开确认页，避免永久 403 死锁。

## 依赖项

- `bifrost-storage::RulesStorage`
- `bifrost-admin::rule_share_import`
- `bifrost-proxy::transform::badge`
- 现有 Admin CORS 与 rules changed 通知链路

## 测试方案

### 单元测试

- `bifrost-storage`：`test_share_env_state_roundtrip_and_clear` 验证 Share 状态文件保存、读取和清理。
- `bifrost-admin`：`import_stashes_pre_share_enabled_rules_once_and_exit_restores` 验证第一次 Share 导入保存快照、第二次 Share 不覆盖快照、退出恢复原 enabled 集合。
- `bifrost-admin`：`exit_share_env_restores_empty_pre_share_enabled_set` 验证进入 Share 前没有 enabled 规则时，退出后不会错误启用任何规则。
- `bifrost-admin`：`exit_share_env_restores_multiple_pre_share_enabled_rules` 验证进入 Share 前多条 enabled 规则时，退出后能全部恢复。
- `bifrost-admin`：`exit_share_env_without_state_is_noop` 验证无 Share 状态时退出是幂等 no-op。
- `bifrost-admin`：handler 单测验证 Exit token 可从 header、JSON body 和 form body 读取，确认页会为旧空 token 状态再生 token，并设置 no-referrer / DENY 安全响应头。
- `bifrost-proxy`：`test_badge_share_env_badge_and_exit_button_present` 验证注入片段包含 Share 红点、呼吸光晕、初始化 Share 状态、`Share preview active` 行、Exit 按钮、真实点击限制、JSON body 退出、成功后刷新当前页面、无二级确认页跳转、无 `no-cors` 盲成功兜底。

### E2E 测试

- `e2e-tests/tests/test_badge_injection_e2e.sh`
  - 创建 `before-enabled`、`before-disabled`、`share-source` 三条 My Rules。
  - 通过真实代理访问包含 `__bifrost_rule` 的分享 URL，断言 GET 重定向到本机规则确认页，且确认页 URL 不再包含原始 `__bifrost_rule`。
  - 调用 `POST /_bifrost/api/rules/share-confirm` 确认应用后，断言返回 clean URL 并进入 Share 环境。
  - 断言 Share 状态 API active，注入页面包含 Share 红点、呼吸光晕、`Share preview active`、原地退出 API、JSON body token 和成功后刷新逻辑，且不包含 `enabled_rule_names`。
  - 连续确认两条 share 链接，断言第二次导入不会覆盖第一次进入 Share 前的恢复快照。
  - 断言无效 token 调用退出 API 返回 403 且 Share 环境仍 active。
  - 从 badge 注入数据读取 token，携带正确 JSON body token 调用退出 API 后断言 `before-enabled` 恢复 enabled，`before-disabled` 保持 disabled，两条导入的 `share/...` 规则均被 disabled。
  - 断言本地确认页对 `Origin: http://localhost:3000` 不返回 `Access-Control-Allow-Origin`，防止被允许的本地业务 origin 读取 token。

### 真实场景测试

- 新增 `human_tests/share-env-exit.md`。
- 使用临时数据目录启动 Bifrost：`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 BIFROST_DATA_DIR=... bifrost start -p <port> --unsafe-ssl --skip-cert-check --no-system-proxy --enable-badge-injection`。
- 通过真实代理访问分享链接，确认跳转到本机规则确认页；用户确认应用后进入 Share 环境，再访问 clean URL，确认页面胶囊出现呼吸光晕和右上角红点，hover panel 显示 `Share preview active` 和 `Exit`。
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
