# Share 环境快速退出

## 背景

Rule Share 是 Bifrost 的分享环境入口：用户从他人分享链接进入本机确认页
(`/_bifrost/share-env/exit` 只是退出兜底，主流程是 `share-confirm`)，确认应用后 Bifrost 会：

- 把分享规则导入 My Rules，命名为 `share/<original>`；
- 用 exclusive 语义启用该规则，同时禁用其他所有 My Rules；
- 保留 Group 规则原状态（本期第一版不参与）。

这套“进入 Share 环境”的语义方便预览别人的规则，但用户需要一个明确的 Share 环境提示与快速退出入口，
退出后必须恢复进入 Share 前的启用规则集合。若没有这层能力，用户很容易在 Share 环境下持续访问真实业务，
误以为自己的本地规则仍在生效。

## 用户目标验证清单

### 必须实现

- 用户确认应用一份 Share 规则后，Bifrost 自动记录一份“进入 Share 前的 enabled My Rules 快照”。
- 该快照只在**首次**进入 Share 环境时写入。已经处于 active Share 环境时，继续确认其它分享链接不覆盖快照。
- Bifrost badge 在注入页面上进入 Share 视觉：胶囊边框呼吸光晕、右上角红点、hover 面板显示
  `Share preview active` 与 `Exit` 按钮。
- 用户点击 `Exit` 或调用退出 API 后，Bifrost 按快照恢复 My Rules 的 enabled 状态，
  清除 `.share_env_state.json`，并触发 rules changed 通知刷新 resolver 与 badge cache。
- 退出成功后当前页面 `location.reload()` 一次，不再打开二级确认页。
- 本地确认页 `/_bifrost/share-env/exit` 保留为兼容入口（form / JSON body 均支持 POST 到 API），
  token 不通过 URL query 传递。
- `GET /_bifrost/api/rules/share-env/status` 返回当前 Share 状态，供 UI / E2E / human tests 校验。

### 必须不破坏

- 现有 Rule Share 导入行为（exclusive enable、命名规则、Group 不参与）保持不变。
- 用户手工在 Rules 页开关 My Rules 的路径不受影响。
- 已有的 Admin CORS 白名单与 rules changed 通知链路不因 Share 退出而放宽。
- 未进入 Share 环境时，注入的 Bifrost badge 视觉、hover 面板、API 行为完全等同实现前。
- 无效或缺失 exit token 时退出 API 严格返回 403 / 409，不静默成功。

### 必须真实验证

- 单元测试覆盖 storage 层 roundtrip、admin handler token 校验、badge 注入片段结构。
- E2E 覆盖“真实代理访问分享链接 → 确认进入 Share → 注入 badge 进入 Share 视觉 → 调 exit API →
  规则恢复”的完整链路，且断言 token 无泄漏、无 CORS 松绑。
- 真实场景手工验证：真实浏览器、真实端口、真实系统代理注入下的呼吸光晕、红点与 `Exit` 按钮。

## 产品语义

### Share 环境是显式、可退出的临时上下文

- 进入：`POST /_bifrost/api/rules/share-confirm`（用户在本机规则确认页点击确认）。
- 判断依据：`.share_env_state.json` 中 `active=true`，字段包含 `imported_rule_name`、`original_share_name`、
  `content_hash`、`pre_share_enabled_rule_names`、`entered_at`、`exit_token`。
- 退出：`POST /_bifrost/api/rules/share-env/exit`，body 含 `exit_token`。
- 状态查询：`GET /_bifrost/api/rules/share-env/status`。

### 快照只写一次

第二份分享链接进入 Share 环境时，`ensure_share_env_state()` 检查已有 `active=true` 的 `.share_env_state.json`：

- 若存在，只更新 `imported_rule_name` 和 `content_hash`，`pre_share_enabled_rule_names` 保持第一次的快照。
- 若不存在，才写入当前 enabled My Rules 名字列表作为快照。

这样保证退出总能回到“进入 Share 系列前”的原始状态，而不是回到上一份 Share。

### Exit token 是安全边界

- Token 只出现在 badge 注入数据的 JSON payload 内和 `.share_env_state.json` 里。
- Token 不通过 URL query 传递（避免 referrer 泄漏）。
- 确认页响应头必须包含 `Cache-Control: no-store`、`Referrer-Policy: no-referrer`、`X-Frame-Options: DENY`。
- 本地确认页 `/_bifrost/share-env/exit` 即使请求来自 CORS 允许的本地 origin（如 `http://localhost:3000`），
  也不返回 `Access-Control-Allow-Origin`，避免浏览器读取 token-bearing HTML。
- `/api/rules/share-env/exit` 提供**专属** CORS 响应：允许业务页面在携带正确 JSON body token 时读取
  成功响应并 `location.reload()`；无 token 或 token 不匹配仍返回 403。
- 远程访问开启时，确认页复用 API 鉴权契约：本机 loopback 免鉴权，远端访问必须具备有效 Admin JWT。

## 技术细节

### 状态文件

- 位置：`<data_dir>/rules/.share_env_state.json`。
- 常量：`SHARE_ENV_STATE_FILE: &str = ".share_env_state.json"`
  （见 `crates/bifrost-storage/src/rules.rs:15`）。
- 结构：`pub struct ShareEnvState { active, imported_rule_name, original_share_name, content_hash,
  pre_share_enabled_rule_names: Vec<String>, entered_at_unix, exit_token }`
  （见 `crates/bifrost-storage/src/rules.rs:296`）。

存储层 API：

- `RulesStorage::load_share_env_state()` — 见 `rules.rs:897`。
- `RulesStorage::save_share_env_state()` — 见 `rules.rs:909`。
- `RulesStorage::clear_share_env_state()` — 见 `rules.rs:918`。

三个方法都必须走 atomic write，避免半写状态阻塞后续退出。

### 进入 Share 快照

`crates/bifrost-admin/src/rule_share_import.rs`：

- `import_rule_share_payload()` (line ~40)：确认应用分享规则，导入并 exclusive 启用。
- `ensure_share_env_state()` (line 183)：判断 `.share_env_state.json` 是否已 active；未 active 时写入
  当前 enabled My Rules 快照 + 新的 `exit_token`；已 active 时只 refresh `imported_rule_name`。

### 退出恢复

`exit_rule_share_env()` (line 123)：

1. `load_share_env_state()`；若为空返回幂等 no-op（`restored=[], disabled=[]`）。
2. 校验入参 `exit_token` 与状态一致，不一致 403；旧状态 token 空返回 409，要求重新打开确认页再生。
3. 遍历现存 My Rules：
   - 属于 `pre_share_enabled_rule_names` → set enabled true；
   - 其它规则（包含刚导入的 `share/<original>`）→ set enabled false。
4. `clear_share_env_state()` 删除状态文件。
5. `notify_after_share_exit()` 触发 rules changed 通知，刷新 resolver 与 badge cache。
6. 返回 `RuleShareExitOutcome { restored_rules, disabled_rules }`。

### Badge 注入

`crates/bifrost-proxy/src/transform/badge.rs`：

- 在注入的初始化 JSON 中新增 `share_env` 字段：`{ active: bool, imported_rule_name: string,
  exit_token: string, exit_endpoint: "/_bifrost/api/rules/share-env/exit" }`。
- `active=true` 时前端片段：
  - 胶囊 border 呼吸光晕（CSS keyframe）。
  - 右上角红点。
  - Hover 面板出现 `Share preview active` 一行 + `Exit` 按钮。
- Exit 按钮点击处理只在**真实用户点击事件**中触发（`isTrusted` 判断），防止脚本静默退出。
- 点击后当前页面直接 `fetch(exit_endpoint, {method:'POST', credentials:'include',
  headers:{'content-type':'application/json'}, body: JSON.stringify({exit_token})})`。
- 成功响应后 `location.reload()`；失败保持 Share 视觉，不做二级页跳转。
- 注入数据**只包含 `exit_token`**，不包含 `pre_share_enabled_rule_names`。

### 确认页兜底

`crates/bifrost-admin/src/router.rs`：

- `admin_path == "/share-env/exit"` 走 `share_env_exit_page` (`handlers/rules.rs`)。
- 该页支持 form body 或 JSON body 提交，token 在 body 中，不在 URL。
- 页面响应头：`Cache-Control: no-store`、`Referrer-Policy: no-referrer`、`X-Frame-Options: DENY`。
- 该页不应用全局 CORS（`apply_share_env_exit_cors` 只作用于 `/api/rules/share-env/exit`）。
- 若旧状态 `exit_token` 为空，确认页会重新生成并持久化 token 后再展示按钮，避免 API 端 409 死锁。

## CLI + Web + Admin API

### Admin API

| Method | Path | 说明 |
| --- | --- | --- |
| `POST` | `/_bifrost/api/rules/share-confirm` | 已存在。确认应用分享规则并进入 Share 环境。 |
| `POST` | `/_bifrost/api/rules/share-env/exit` | 新增。JSON body: `{ exit_token }`。成功返回 `{ restored_rules, disabled_rules }`。 |
| `GET`  | `/_bifrost/api/rules/share-env/status` | 新增。返回 `{ active, imported_rule_name, original_share_name, entered_at_unix }`；token 不返回。 |
| `GET`  | `/_bifrost/share-env/exit` | 已存在（兜底确认页）。form / JSON body POST 到 exit API。 |

CORS：

- `/api/rules/share-env/exit` 有专属 CORS 允许业务页面读取成功响应（见 `router.rs:81`
  `apply_share_env_exit_cors`）。
- `/share-env/exit` **不**返回 CORS 头。
- 远程访问时，确认页与 exit API 同样受 `check_api_auth` 保护。

### Web UI（注入 badge 之外）

Rules 页顶部横幅：`active=true` 时展示“You are in a Share preview environment. Exit”按钮，
点击后走同一个 exit API。这是给非注入页面场景的入口，行为与 badge 内 Exit 一致。

### CLI

第一版**不新增** CLI 子命令。Share 环境是一个偏 GUI/浏览器的能力，CLI 用户可通过：

```
curl -X POST http://127.0.0.1:9900/_bifrost/api/rules/share-env/exit \
  -H 'content-type: application/json' \
  -d '{"exit_token": "<token-from-status-file>"}'
```

在极端排障时手工触发。

## Sync 边界

- `.share_env_state.json` 是**本地状态文件**，不进入任何 sync / share / export 链路。
- exit token 只落地在本地文件与本机注入 payload，不通过 Sync manager 上传或推送。
- Share 环境退出触发的 rules changed 通知**只在本机**广播，不推送到远端；远端订阅者只能感知
  My Rules enabled 状态变化，不看到 Share 状态本身。
- Group 规则本期不参与 Share 环境语义；`disabled_rules` 只包含 My Rules，不动 Group。

## 实现切分

### Phase 1：Storage + Import 快照

- 新增 `.share_env_state.json`、`ShareEnvState`、三个 storage API。
- `import_rule_share_payload` / `ensure_share_env_state` 首次写入 + 后续不覆盖。
- 单元测试：`test_share_env_state_roundtrip_and_clear`、
  `import_stashes_pre_share_enabled_rules_once_and_exit_restores`。

### Phase 2：Exit API + 通知

- 新增 `exit_rule_share_env()`、`GET /status`。
- 拼接 rules changed 通知路径。
- Token 校验：header / JSON body / form body 都可读；空 token 状态 409 提示重新打开确认页。

### Phase 3：注入 Badge + Exit 按钮

- 注入 payload 新增 `share_env`。
- Share 视觉：呼吸光晕、红点、Exit 按钮。
- Exit 逻辑：真实点击、JSON body、成功后 `location.reload()`、不走二级页。

### Phase 4：安全响应头 + CORS 拆分

- 确认页 `Cache-Control / Referrer-Policy / X-Frame-Options`。
- `/api/rules/share-env/exit` 专属 CORS；`/share-env/exit` 无 CORS。
- Remote 场景下的 Admin JWT 校验路径贯通。

## 测试方案

### 单元测试

- `crates/bifrost-storage`
  - `test_share_env_state_roundtrip_and_clear` (`rules.rs:1255`)
- `crates/bifrost-admin`
  - `import_stashes_pre_share_enabled_rules_once_and_exit_restores`
  - `exit_share_env_restores_empty_pre_share_enabled_set`
  - `exit_share_env_restores_multiple_pre_share_enabled_rules`
  - `exit_share_env_without_state_is_noop`
  - `exit_share_env_rejects_invalid_token_403`
  - `exit_share_env_returns_409_when_token_empty_prompts_reopen`
  - handler 单测：token 可从 header / JSON body / form body 读取；确认页会为旧空 token 状态再生
    token；响应头 no-store / no-referrer / DENY 存在。
- `crates/bifrost-proxy`
  - `test_badge_share_env_badge_and_exit_button_present`
    - 断言注入片段含 Share 红点、呼吸光晕、`Share preview active`、Exit 按钮、真实点击限制、
      JSON body 退出、成功后刷新、无二级页跳转、无 `no-cors` 盲成功兜底。
  - `test_badge_payload_excludes_pre_share_enabled_rule_names`

### E2E 测试

`e2e-tests/tests/test_badge_injection_e2e.sh`（已存在）新增/更新用例：

1. 创建 `before-enabled`、`before-disabled`、`share-source` 三条 My Rules。
2. 通过真实代理访问包含 `__bifrost_rule` 的分享 URL，断言 GET 重定向到本机规则确认页，
   且确认页 URL 不再包含原始 `__bifrost_rule`。
3. `POST /_bifrost/api/rules/share-confirm` 后返回 clean URL，进入 Share 环境。
4. 断言 `GET /_bifrost/api/rules/share-env/status` `active=true`，注入页面包含 Share 红点、
   呼吸光晕、`Share preview active`、原地退出 API 与 JSON body token，不包含 `enabled_rule_names`。
5. 连续确认两条 share 链接，断言第二次不覆盖第一次进入 Share 前的快照。
6. 无效 token 调 exit API 返回 403 且 Share 仍 active。
7. 从 badge 注入数据读取 token，用正确 JSON body 调 exit，断言 `before-enabled` 恢复 enabled、
   `before-disabled` 保持 disabled、两条导入的 `share/...` 规则被 disabled。
8. 断言本地确认页对 `Origin: http://localhost:3000` 不返回 `Access-Control-Allow-Origin`。

### 真实场景测试

`human_tests/share-env-exit.md`（已存在）覆盖：

- 用临时数据目录启动 Bifrost：
  ```
  BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 \
  BIFROST_DATA_DIR=... bifrost start -p <port> --unsafe-ssl \
  --skip-cert-check --no-system-proxy --enable-badge-injection
  ```
- 通过真实代理访问分享链接 → 确认页 → 应用 → 访问 clean URL → 观察胶囊呼吸光晕与红点、
  hover 面板中的 `Share preview active` 与 `Exit`。
- 点击 Exit，确认规则状态恢复；再次访问同一页面胶囊回到正常视觉。
- 覆盖 Chrome / Safari 两款浏览器。

## Review / Fix / Test 闭环

- 第 1 轮：复核 share 导入 + 状态持久化 + badge JSON + 退出 API diff；跑 storage / admin / proxy
  窄单测与 E2E share 用例。
- 第 2 轮：复查第 1 轮后的 diff、`human_tests` 索引与真实场景步骤；复跑相关单测、E2E 与 human tests。
- 校验命令：
  - `cargo test -p bifrost-storage share_env -- --nocapture`
  - `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin rule_share -- --nocapture`
  - `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-proxy badge_share_env -- --nocapture`
  - `bash e2e-tests/tests/test_badge_injection_e2e.sh`
  - `cargo test --workspace --all-features`
  - `cargo fmt --all -- --check`
  - `cargo clippy --all-targets --all-features -- -D warnings`

## 风险与决策

- **风险：token 通过日志或 referrer 泄漏**。缓解：token 只走 body、页面头 `Referrer-Policy: no-referrer`、
  日志脱敏、注入 payload 不落 URL。
- **风险：Share 快照与真实 enabled 状态不一致（例如 Share 期间用户手工切换规则）**。缓解：本期
  按“进入前快照恢复”为准，忽略 Share 期间的用户切换；文档明确说明；未来可考虑“智能合并”但不在
  本期范围。
- **风险：`.share_env_state.json` 手工损坏或删除**。缓解：`load_share_env_state()` 遇到解析失败返回
  `None`，进入 Share 前必须 `save_share_env_state()` 成功才认为进入；`clear_share_env_state()` 幂等。
- **风险：CORS 松绑导致 token 泄漏**。缓解：CORS 只作用于 `/api/rules/share-env/exit`，且必须携带
  正确 body token；确认页面 `/share-env/exit` 严格禁 CORS。
- **决策：Group 规则本期不参与 Share 快照**。理由：Group 语义与 exclusive 冲突且 Group 生命周期
  独立管理；后续独立设计。
- **决策：exit API 幂等**。理由：多标签页并发点击 Exit 时不产生错误 UX；无状态时返回空 restored/disabled。

## 文档更新要求

- 不新增 CLI 参数、README 协议列表或 Hook。
- 更新 `human_tests/readme.md`，新增 Share 环境退出真实场景索引。
- 若后续引入 Group 参与 Share，需要在本文档追加“Group 边界”一节。
