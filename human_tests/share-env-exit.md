# Share 环境快速退出测试用例

## 功能模块说明

验证通过 Rule Share 确认页进入预览环境后，Bifrost 注入到业务页面的页面胶囊能用轻量视觉状态提示当前处于 Share 环境，并提供退出入口。退出后必须恢复进入 Share 前启用的 My Rules 状态。

## 前置条件

- 使用临时数据目录，禁止污染本机默认 Bifrost 数据。
- 启动 Bifrost 时必须设置：
  - `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`
  - `BIFROST_DISABLE_TRAY=1`
  - `--no-system-proxy`
  - `--enable-badge-injection`
- 准备一个本地 HTML 上游服务，例如 `e2e-tests/test_data/badge_injection/index.html`。

参考启动命令：

```bash
BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
BIFROST_DISABLE_TRAY=1 \
BIFROST_DATA_DIR=./.bifrost-human-share-env \
target/release/bifrost start -p 18880 --unsafe-ssl --skip-cert-check --no-system-proxy --enable-badge-injection
```

## 测试用例列表

### TC-SEE-01：进入 Share 环境后页面胶囊展示 Share 视觉状态和 Exit

**操作步骤**：
1. 启动本地 HTML 服务：
   ```bash
   python3 -m http.server 18881 --bind 127.0.0.1 --directory e2e-tests/test_data/badge_injection
   ```
2. 创建三条 My Rules：
   - `before-enabled`：enabled=true
   - `before-disabled`：enabled=false
   - `share-source`：enabled=false
3. 调用 `POST /_bifrost/api/rules/share-link` 为 `share-source` 生成指向 `http://127.0.0.1:18881/index.html` 的 share URL。
4. 通过 Bifrost 代理访问 share URL，记录 302 `Location`。
5. 确认 `Location` 指向 `http://127.0.0.1:18880/_bifrost/share/rule?...`，从 query 中取出 `payload` 和 `target`。
6. 调用 `POST /_bifrost/api/rules/share-confirm`，body 为 `{"payload":"<payload>","target_url":"<target>"}`。
7. 再通过代理访问 clean URL `http://127.0.0.1:18881/index.html`。
8. 在浏览器中打开响应页面，悬浮页面胶囊。

**预期结果**：
- 第一次访问 share URL 返回本机规则确认页 redirect，确认页 URL 不再包含原始 `__bifrost_rule`。
- 确认应用后返回 clean URL，`GET /_bifrost/api/rules/share-env/status` 返回 `active=true` 和 `requested_name=share-source`。
- 页面胶囊不显示被裁剪的 `Share` 文字角标，B 胶囊边框有呼吸光晕，右上角显示红色圆点。
- hover panel 中显示短文案 `Share preview active`。
- 同一行存在 `Exit` 按钮。
- 注入数据包含退出用 `exit_token`，但不包含恢复快照 `enabled_rule_names`。
- Exit 动作直接调用 `/_bifrost/api/rules/share-env/exit`，以 JSON body 传递 token，成功后刷新当前页面；不打开 `/_bifrost/share-env/exit` 二级确认页，退出脚本不包含 `mode:'no-cors'`。

### TC-SEE-02：退出 Share 环境后恢复进入前启用规则

**操作步骤**：
1. 在 TC-SEE-01 的 Share 环境中点击 hover panel 的 `Exit` 按钮。
2. 或从注入页面 HTML 中读取 `"exit_token":"..."` 后调用：
   ```bash
   curl -X POST "http://127.0.0.1:18880/_bifrost/api/rules/share-env/exit" \
     -H "Content-Type: application/json" \
     --data '{"token":"<exit_token>"}'
   ```
3. 查询三条规则状态：
   ```bash
   curl http://127.0.0.1:18880/_bifrost/api/rules/before-enabled
   curl http://127.0.0.1:18880/_bifrost/api/rules/before-disabled
   curl http://127.0.0.1:18880/_bifrost/api/rules/share%2Fshare-source
   ```
4. 查询 Share 环境状态：
   ```bash
   curl http://127.0.0.1:18880/_bifrost/api/rules/share-env/status
   ```

**预期结果**：
- Exit API 返回 `was_active=true`。
- 无效或缺失 body/header token 的 Exit API 返回 `403`，Share 环境保持 active。
- 点击 `Exit` 后当前业务页面刷新，不打开新的确认窗口或确认页。
- `before-enabled.enabled=true`。
- `before-disabled.enabled=false`。
- `share/share-source.enabled=false`。
- Share 环境状态返回 `active=false`。
- 后续通过代理刷新 clean URL 时，页面仍可显示普通 Bifrost badge，但不再处于 active Share 状态，不再显示红点和呼吸光晕。

### TC-SEE-03：连续打开多个 Share 链接不覆盖原始恢复快照

**操作步骤**：
1. 进入第一个 Share 链接前，只启用 `before-enabled`。
2. 打开 `share-source` 的 Share 链接，进入本机规则确认页并确认应用，确认进入 Share 环境。
3. 再创建并打开第二条分享规则 `share-source-2` 的 Share 链接，进入本机规则确认页并确认应用。
4. 点击 hover panel 的 `Exit`，或用注入页面里的 `exit_token` 通过 JSON body 调用 Exit API。
5. 查询规则状态。

**预期结果**：
- 第二次 Share 导入不会覆盖第一次进入 Share 前的快照。
- 退出后仍只恢复 `before-enabled.enabled=true`。
- 两条导入的 `share/...` 规则均为 disabled。
- 用户会回到第一次点击任何 Share 链接之前的 My Rules enabled 状态。

### TC-SEE-04：进入 Share 前没有启用规则时退出后保持全禁用

**操作步骤**：
1. 创建两条 My Rules：
   - `before-disabled`：enabled=false
   - `share-source`：enabled=false
2. 生成并访问 `share-source` 的 Share 链接，进入本机规则确认页并确认应用。
3. 确认 `share/share-source.enabled=true`，`before-disabled.enabled=false`。
4. 点击 hover panel 的 `Exit`，或用注入页面里的 `exit_token` 通过 JSON body 调用 Exit API。
5. 查询 `before-disabled` 和 `share/share-source` 的状态。

**预期结果**：
- Exit API 返回 `was_active=true`，`restored_rules=[]`。
- `before-disabled.enabled=false`。
- `share/share-source.enabled=false`。
- Share 环境状态返回 `active=false`。

### TC-SEE-05：进入 Share 前多条规则启用时退出后全部恢复

**操作步骤**：
1. 创建四条 My Rules：
   - `before-a`：enabled=true
   - `before-b`：enabled=true
   - `before-disabled`：enabled=false
   - `share-source`：enabled=false
2. 生成并访问 `share-source` 的 Share 链接，进入本机规则确认页并确认应用。
3. 确认 `before-a.enabled=false`、`before-b.enabled=false`、`share/share-source.enabled=true`。
4. 点击 hover panel 的 `Exit`，或用注入页面里的 `exit_token` 通过 JSON body 调用 Exit API。
5. 查询四条规则状态。

**预期结果**：
- Exit API 返回 `was_active=true`，`restored_rules` 包含 `before-a` 和 `before-b`。
- `before-a.enabled=true`。
- `before-b.enabled=true`。
- `before-disabled.enabled=false`。
- `share/share-source.enabled=false`。
- Share 环境状态返回 `active=false`。

## 清理步骤

```bash
pkill -f "bifrost start -p 18880" || true
pkill -f "http.server 18881" || true
rm -rf ./.bifrost-human-share-env
```

## 执行记录

- 2026-06-22：已通过 `e2e-tests/tests/test_badge_injection_e2e.sh` 的 Share 环境 case 自动执行 TC-SEE-01 和 TC-SEE-02 的 API、代理、badge 注入内容与规则恢复断言。
- 2026-06-22：已使用 Playwright 真实浏览器通过 Bifrost 代理打开 share URL，确认 `#__bifrost_badge__` 立即带 `--share` 状态、右上角红点和呼吸光晕，hover panel 显示 `Share preview active` 和 `Exit`；点击 Exit 后验证 `share-env/status.active=false`、`before-enabled.enabled=true`、`share/share-source.enabled=false`。
- 2026-06-23（旧版确认页实现记录）：根据 PR review 更新预期：被代理页面注入数据不得包含 `exit_token`，Exit 打开本地确认页，token 通过 JSON/form body 提交；连续打开多个 share 链接后退出必须恢复第一次 Share 前的 My Rules enabled 状态。
- 2026-06-23（旧版确认页实现记录）：已使用临时数据目录 `.bifrost-human-share-env` 启动本地 Bifrost 和 HTML 服务，用 Google Chrome/Playwright 通过真实代理打开 clean URL，确认 Share badge 红点、呼吸光晕、hover panel `Share preview active` 和 `Exit`；确认被代理页面 visible DOM 不包含 `exit_token`；再打开本地确认页点击 `Exit share preview`，验证连续两个 Share 链接后退出恢复到第一次 Share 前状态，`before-enabled.enabled=true`、`before-disabled.enabled=false`、两条 `share/...` 规则均为 disabled。测试结束已清理临时进程和数据目录。
- 2026-06-23：根据 Codex review 更新安全预期：本地确认页不应用全局 CORS；被代理页面注入数据不得包含 `enabled_rule_names` 恢复快照。已通过 E2E 对 `Origin: http://localhost:3000` 的确认页响应断言无 `Access-Control-Allow-Origin`，并断言 badge inline data 不包含 `enabled_rule_names`，避免 allowed local origin 读取退出 token 或业务页面读取进入 Share 前的规则名。
- 2026-06-23：根据用户反馈更新预期：Exit 不再打开二级确认页；badge 在当前页面以 JSON body 携带 `exit_token` 调用 Exit API，确认成功响应后刷新当前页面，并补充进入前空 enabled / 多 enabled 的恢复边界用例。
- 2026-06-23：根据 Rule Share P1 确认页语义更新进入 Share 环境步骤：share URL 先 302 到 `/_bifrost/share/rule`，确认 `share-confirm` 后才进入 Share 环境。已执行 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 SKIP_BUILD=true bash e2e-tests/tests/test_badge_injection_e2e.sh`，73/73 通过，覆盖确认页应用、Share badge、连续两条 Share 链接、无效/有效 exit token 和规则恢复。
