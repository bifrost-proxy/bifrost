# Sync API 测试用例

## 功能模块说明

Bifrost Sync API 提供云端同步管理功能，包括同步状态查询、配置更新、登录/登出、手动触发同步、会话管理、远程配置采样等。此外，`/api/env/*`、`/api/room/*`、`/api/user/*` 作为同步代理转发端点，需要在已登录状态下将请求代理到远程服务的 `/v4/` 对应路径。

## 前置条件

1. 启动 Bifrost 服务（使用临时数据目录避免污染正式环境）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
   ```
2. 确保端口 8800 可用且未被其他服务占用
3. 部分用例需要有效的远程同步服务地址（默认配置即可）

---

## 测试用例

### TC-ASN-01：获取初始同步状态

**操作步骤**：
1. 执行：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/sync/status | jq .
   ```

**预期结果**：
- 返回 HTTP 200
- 响应为 JSON 对象，包含以下字段：
  - `enabled`：布尔值（初始为 `true`）
  - `auto_sync`：布尔值
  - `remote_base_url`：字符串（远程服务地址）
  - `has_session`：`false`（未登录）
  - `reachable`：布尔值
  - `authorized`：`false`
  - `syncing`：`false`
  - `reason`：`"unauthorized"`、`"unreachable"` 或其他当前同步运行状态（取决于启动登录预检和远端可达性）
  - `last_sync_at`：`null`
  - `last_sync_action`：`null`
  - `last_error`：`null` 或字符串
  - `user`：`null`（未登录）

---

### TC-ASN-02：使用错误 HTTP 方法访问同步状态

**操作步骤**：
1. 执行：
   ```bash
   curl -s -o /dev/null -w "%{http_code}" -X POST http://127.0.0.1:8800/_bifrost/api/sync/status
   ```

**预期结果**：
- 返回 HTTP 405（Method Not Allowed）

---

### TC-ASN-03：更新同步配置 — 开启同步

**操作步骤**：
1. 执行：
   ```bash
   curl -s -X PUT http://127.0.0.1:8800/_bifrost/api/sync/config \
     -H "Content-Type: application/json" \
     -d '{"enabled": true}' | jq .
   ```

**预期结果**：
- 返回 HTTP 200
- 响应中 `enabled` 为 `true`
- `reason` 不再是 `"disabled"`（可能为 `"reachable"`、`"unreachable"` 或 `"unauthorized"` 等，取决于远程服务可达性）

---

### TC-ASN-04：更新同步配置 — 修改多个字段

**操作步骤**：
1. 执行：
   ```bash
   curl -s -X PUT http://127.0.0.1:8800/_bifrost/api/sync/config \
     -H "Content-Type: application/json" \
     -d '{"auto_sync": true, "probe_interval_secs": 30, "connect_timeout_ms": 5000}' | jq .
   ```

**预期结果**：
- 返回 HTTP 200
- 响应中 `auto_sync` 为 `true`

---

### TC-ASN-05：更新同步配置 — remote_base_url 为空字符串被拒绝

**操作步骤**：
1. 执行：
   ```bash
   curl -s -X PUT http://127.0.0.1:8800/_bifrost/api/sync/config \
     -H "Content-Type: application/json" \
     -d '{"remote_base_url": ""}' | jq .
   ```

**预期结果**：
- 返回 HTTP 400
- 响应包含错误信息 `"remote_base_url cannot be empty"`

---

### TC-ASN-06：更新同步配置 — remote_base_url 非法 URL 被拒绝

**操作步骤**：
1. 执行：
   ```bash
   curl -s -X PUT http://127.0.0.1:8800/_bifrost/api/sync/config \
     -H "Content-Type: application/json" \
     -d '{"remote_base_url": "not-a-valid-url"}' | jq .
   ```

**预期结果**：
- 返回 HTTP 400
- 响应包含错误信息 `"remote_base_url must be a valid URL"`

---

### TC-ASN-07：更新同步配置 — 无效 JSON 请求体

**操作步骤**：
1. 执行：
   ```bash
   curl -s -X PUT http://127.0.0.1:8800/_bifrost/api/sync/config \
     -H "Content-Type: application/json" \
     -d 'invalid-json' | jq .
   ```

**预期结果**：
- 返回 HTTP 400
- 响应包含 `"Invalid JSON"` 错误信息

---

### TC-ASN-08：使用错误 HTTP 方法更新同步配置

**操作步骤**：
1. 执行：
   ```bash
   curl -s -o /dev/null -w "%{http_code}" -X POST http://127.0.0.1:8800/_bifrost/api/sync/config
   ```

**预期结果**：
- 返回 HTTP 405（Method Not Allowed）

---

### TC-ASN-09：触发手动同步

**操作步骤**：
1. 执行：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/sync/run | jq .
   ```

**预期结果**：
- 返回 HTTP 200
- 响应为最新的 sync status JSON 对象
- `syncing` 字段可能短暂为 `true`（取决于同步执行速度）

---

### TC-ASN-10：使用错误 HTTP 方法触发手动同步

**操作步骤**：
1. 执行：
   ```bash
   curl -s -o /dev/null -w "%{http_code}" -X GET http://127.0.0.1:8800/_bifrost/api/sync/run
   ```

**预期结果**：
- 返回 HTTP 405（Method Not Allowed）

---

### TC-ASN-11：保存会话 token

**操作步骤**：
1. 执行：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/sync/session \
     -H "Content-Type: application/json" \
     -d '{"token": "test-session-token-abc123"}' | jq .
   ```

**预期结果**：
- 返回 HTTP 200 或 HTTP 500（取决于 token 是否能被远程服务验证）
- 如果 token 有效，响应为 sync status，其中 `has_session` 为 `true`
- 如果 token 无效，返回错误信息

---

### TC-ASN-12：保存会话 token — 空 token 被拒绝

**操作步骤**：
1. 执行：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/sync/session \
     -H "Content-Type: application/json" \
     -d '{"token": ""}' | jq .
   ```

**预期结果**：
- 返回 HTTP 400
- 响应包含错误信息 `"token is required"`

---

### TC-ASN-13：保存会话 token — 仅空格 token 被拒绝

**操作步骤**：
1. 执行：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/sync/session \
     -H "Content-Type: application/json" \
     -d '{"token": "   "}' | jq .
   ```

**预期结果**：
- 返回 HTTP 400
- 响应包含错误信息 `"token is required"`

---

### TC-ASN-14：获取登录 URL

**操作步骤**：
1. 执行：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/api/sync/login-url?callback_url=http://127.0.0.1:8800/_bifrost/public/sync-login" | jq .
   ```

**预期结果**：
- 返回 HTTP 200
- 响应为 JSON 对象，包含 `login_url` 字段
- `login_url` 为一个有效的 URL 字符串，指向远程登录页面

---

### TC-ASN-15：获取登录 URL — 缺少 callback_url 参数

**操作步骤**：
1. 执行：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/sync/login-url | jq .
   ```

**预期结果**：
- 返回 HTTP 400
- 响应包含 `"Invalid query"` 错误信息

---

### TC-ASN-16：获取远程配置采样

**操作步骤**：
1. 执行：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/api/sync/remote-sample?limit=5" | jq .
   ```

**预期结果**：
- 如果已登录远程服务：返回 HTTP 200，响应为 JSON 数组，最多包含 5 条远程配置采样数据
- 如果未登录远程服务：返回 HTTP 500，响应包含错误信息

---

### TC-ASN-17：获取远程配置采样 — 不带 limit 参数使用默认值

**操作步骤**：
1. 执行：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/sync/remote-sample | jq .
   ```

**预期结果**：
- 与 TC-ASN-16 类似，但使用默认 limit 值 10
- 响应格式一致

---

### TC-ASN-18：请求同步登录（打开浏览器）

**操作步骤**：
1. 执行：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/sync/login | jq .
   ```

**预期结果**：
- 返回 HTTP 200（即使浏览器无法打开，API 本身应返回当前同步状态）
- 响应为 sync status JSON 对象
- 如果无法打开浏览器，返回 HTTP 500 和 `"Failed to open sync login page"` 相关错误信息

---

### TC-ASN-19：执行登出

**操作步骤**：
1. 执行：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/sync/logout | jq .
   ```

**预期结果**：
- 返回 HTTP 200
- 响应为 sync status JSON 对象
- `has_session` 变为 `false`
- `user` 变为 `null`
- `authorized` 变为 `false`

---

### TC-ASN-20：登出后再次查询同步状态确认已清除

**操作步骤**：
1. 先执行登出：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/sync/logout > /dev/null
   ```
2. 查询状态：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/sync/status | jq .
   ```

**预期结果**：
- `has_session` 为 `false`
- `user` 为 `null`
- `authorized` 为 `false`

---

### TC-ASN-21：访问不存在的 sync 子路径

**操作步骤**：
1. 执行：
   ```bash
   curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:8800/_bifrost/api/sync/nonexistent
   ```

**预期结果**：
- 返回 HTTP 404

---

### TC-ASN-22：env 代理转发 — 未登录时请求失败

**前置条件**：未进行同步登录（`has_session` 为 `false`）

**操作步骤**：
1. 执行：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/env/list | jq .
   ```

**预期结果**：
- 返回 HTTP 502（Bad Gateway）
- 响应包含 `"Failed to proxy env request"` 相关错误信息
- 表明因未登录远程服务，代理转发失败

---

### TC-ASN-23：room 代理转发 — 未登录时请求失败

**前置条件**：未进行同步登录

**操作步骤**：
1. 执行：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/room/list | jq .
   ```

**预期结果**：
- 返回 HTTP 502（Bad Gateway）
- 响应包含 `"Failed to proxy room request"` 相关错误信息

---

### TC-ASN-24：user 代理转发 — 未登录时请求失败

**前置条件**：未进行同步登录

**操作步骤**：
1. 执行：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/user/me | jq .
   ```

**预期结果**：
- 返回 HTTP 502（Bad Gateway）
- 响应包含 `"Failed to proxy user request"` 相关错误信息

---

### TC-ASN-25：room 代理转发 — POST /api/room 适配为 POST /v4/group/invite

**前置条件**：未进行同步登录（此用例验证请求体适配逻辑，即使远程不可达也能验证请求体校验）

**操作步骤**：
1. 执行：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/room \
     -H "Content-Type: application/json" \
     -d '{"group_id": "g123", "user_id": "u456", "level": 1}' | jq .
   ```

**预期结果**：
- 如果远程服务不可达，返回 HTTP 502 和 `"Failed to proxy room request"` 相关错误
- 请求不会返回 400（说明请求体适配逻辑 `adapt_create_room_to_invite` 正常工作）

---

### TC-ASN-26：room 代理转发 — POST /api/room 缺少必要字段被拒绝

**操作步骤**：
1. 执行：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/room \
     -H "Content-Type: application/json" \
     -d '{"invalid_field": "value"}' | jq .
   ```

**预期结果**：
- 返回 HTTP 400
- 响应包含 `"Invalid create room request body"` 错误信息
- 因为缺少 `group_id` 和 `user_id` 必要字段

---

### TC-ASN-27：env/room/user 代理转发 — 不支持的 HTTP 方法

**操作步骤**：
1. 执行：
   ```bash
   curl -s -o /dev/null -w "%{http_code}" -X OPTIONS http://127.0.0.1:8800/_bifrost/api/env/list
   ```

**预期结果**：
- 返回 HTTP 200（因为 OPTIONS 请求在路由层被 CORS preflight 处理）

---

### TC-ASN-28：公开端点 — sync-login 回调页面（无 token）

**操作步骤**：
1. 执行：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/public/sync-login
   ```

**预期结果**：
- 返回 HTTP 200
- 响应为 HTML 页面
- 页面标题包含 `"Bifrost Remote Sign-In"`
- 页面内容包含 `"Missing login token from remote callback."`
- 页面显示错误状态（`Remote Sign-In Failed`）

---

### TC-ASN-29：公开端点 — sync-login 回调页面（带无效 token）

**操作步骤**：
1. 执行：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/public/sync-login?token=invalid-token-xyz"
   ```

**预期结果**：
- 返回 HTTP 200
- 响应为 HTML 页面
- 如果 token 保存失败，页面显示 `"Remote Sign-In Failed"` 和 `"Failed to save sync session"` 相关信息
- 如果 token 保存成功（格式被接受），页面显示 `"Login completed. You can close this window now."` 并包含自动重定向脚本

---

### TC-ASN-30：更新同步配置 — 设置有效的 remote_base_url

**操作步骤**：
1. 执行：
   ```bash
   curl -s -X PUT http://127.0.0.1:8800/_bifrost/api/sync/config \
     -H "Content-Type: application/json" \
     -d '{"remote_base_url": "https://example.com/api"}' | jq .
   ```
2. 验证配置已更新：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/sync/status | jq .remote_base_url
   ```

**预期结果**：
- 第一步返回 HTTP 200
- 第二步返回 `"https://example.com/api"`
- remote_base_url 已成功更新

---

### TC-ASN-31：CI/沙箱环境 token-only 默认 Provider 与 token + URL 直登

**操作步骤**：
1. 使用非 9900 动态端口启动本地 mock sync server，提供以下接口：
   - `GET /v4/sso/check` 返回 200
   - `GET /v4/sso/info` 在请求头 `x-bifrost-token: ci-token` 时返回用户 `ci-user`
2. 使用临时数据目录和非 9900 端口启动 Bifrost：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d)" target/release/bifrost -p <admin_port> start -y --access-mode allow_all --skip-cert-check --unsafe-ssl --no-system-proxy
   ```
3. 先验证只提供 token 时使用内置默认 Provider：
   ```bash
   BIFROST_DATA_DIR="<same_temp_dir>" target/release/bifrost -p <admin_port> sync login --token ci-token-default
   ```
4. 查询 `sync login --help`：
   ```bash
   target/release/bifrost sync login --help
   ```
5. 将同步配置指向 mock sync server：
   ```bash
   BIFROST_DATA_DIR="<same_temp_dir>" target/release/bifrost -p <admin_port> sync config --remote-url "http://127.0.0.1:<mock_sync_port>"
   ```
6. 执行 token-only 直登命令：
   ```bash
   BIFROST_DATA_DIR="<same_temp_dir>" target/release/bifrost -p <admin_port> sync login --token ci-token
   ```
7. 执行显式 URL 直登命令：
   ```bash
   BIFROST_DATA_DIR="<same_temp_dir>" target/release/bifrost -p <admin_port> sync login --token ci-token --url "http://127.0.0.1:<mock_sync_port>"
   ```
8. 查询同步状态：
   ```bash
   curl -s "http://127.0.0.1:<admin_port>/_bifrost/api/sync/status" | jq .
   ```

**预期结果**：
- 第 3 步 CLI 输出包含 `"Login successful."`
- 第 3 步 CLI 输出包含默认远端 URL `"https://bifrost.bytedance.net"`
- 第 4 步 help 输出包含 token 获取地址 `https://bifrost.bytedance.net/v4/sso/token-login`
- 第 6 步 CLI 输出包含 `"Login successful."`
- 第 6 步 CLI 输出包含 mock sync URL，说明 token-only 走当前同步配置
- 第 7 步 CLI 输出包含 `"Login successful."`
- 第 7 步成功后查询状态确认 `remote_base_url` 为 mock sync URL 或内置默认 URL。
- 状态 JSON 中 `has_session` 为 `true`
- 后台同步完成后 `authorized` 为 `true`
- 状态 JSON 中 `remote_base_url` 等于传入 URL
- 状态 JSON 中 `user.user_id` 为 `"ci-user"`
- 全流程不打开浏览器，不依赖 SSO 回调页面

---

### TC-ASN-32：CI/沙箱环境直登 — token-only API 成功，URL-only 返回 400

**操作步骤**：
1. 在临时数据目录和非 9900 端口启动 Bifrost，并将 sync config 指向 mock sync server。
2. 执行：
   ```bash
   curl -s -o /tmp/bifrost-sync-login-token-only.json -w "%{http_code}" \
     -X POST "http://127.0.0.1:<admin_port>/_bifrost/api/sync/login" \
     -H "Content-Type: application/json" \
     -d '{"token":"ci-token"}'
   ```
3. 查看响应体：
   ```bash
   cat /tmp/bifrost-sync-login-token-only.json
   ```
4. 执行 URL-only 请求：
   ```bash
   curl -s -o /tmp/bifrost-sync-login-url-only.json -w "%{http_code}" \
     -X POST "http://127.0.0.1:<admin_port>/_bifrost/api/sync/login" \
     -H "Content-Type: application/json" \
     -d '{"remote_base_url":"http://127.0.0.1:<mock_sync_port>"}'
   ```
5. 查看响应体：
   ```bash
   cat /tmp/bifrost-sync-login-url-only.json
   ```

**预期结果**：
- token-only 请求 HTTP 状态码为 200
- token-only 响应体中 `remote_base_url` 等于当前同步配置的 mock sync URL
- token-only 请求写入 sync session token，不要求 `remote_base_url`
- URL-only 请求 HTTP 状态码为 400
- URL-only 响应体包含 `"token is required"`
- URL-only 请求不会写入新的 sync session token

---

### TC-ASN-33：启动登录预检 — 默认开启、可达自动弹一次、重启不重复、不可达不弹

**操作步骤**：
1. 执行隔离 E2E 脚本：
   ```bash
   bash e2e-tests/tests/test_sync_startup_login_preflight_e2e.sh
   ```
2. 脚本会启动一个本地 mock sync server，使用临时数据目录写入：
   ```toml
   [sync]
   enabled = true
   auto_sync = true
   remote_base_url = "http://127.0.0.1:<mock_sync_port>"
   probe_interval_secs = 2
   connect_timeout_ms = 500
   ```
3. 脚本会设置 `BIFROST_SYNC_LOGIN_BROWSER_DRY_RUN_FILE`，以文件记录“本来要打开的登录 URL”，避免真实弹浏览器。
4. 第一次启动 Bifrost，等待 dry-run 文件出现 1 条 `/v4/sso/logout?next=...` 登录 URL，并检查 `sync-state.json` 写入 `startup_login_prompt`。
5. 使用同一临时数据目录重启 Bifrost，等待服务 ready 后确认 dry-run 文件仍只有 1 条登录 URL。
6. 再使用另一个临时数据目录，把 `remote_base_url` 指向不可达本地端口，并设置较短的 `BIFROST_SYNC_STARTUP_LOGIN_PREFLIGHT_RETRY_DELAY_MS`。
7. 等待足够覆盖三次短间隔启动预检后，确认不可达场景没有写入登录 URL。

**预期结果**：
- 新安装默认 Sync 配置为启用状态，启动登录预检会在无 token 时运行。
- 可达 + 无 token + 从未自动弹过时，只自动打开 1 次登录 URL。
- 自动打开后 `sync-state.json` 持久化 `startup_login_prompt`，后续重启不再自动打开登录 URL。
- 不可达 + 无 token 时最多探测 3 次；3 次仍不可达时不自动打开登录 URL。
- 未登录状态下不会继续按 `probe_interval_secs` 高频探测并反复弹窗。
- 启动预检处于重试等待时，如果用户完成 token 登录，登录 wake 应立即打断预检等待，后台状态应及时变为 authorized。
- 手动登录入口不受本用例限制，用户仍可主动执行 `bifrost sync login`。

**真实执行记录**：
- 2026-06-02：执行 `bash e2e-tests/tests/test_sync_startup_login_preflight_e2e.sh` 通过。脚本使用临时数据目录、本地 mock sync server、`--no-system-proxy` 和 `BIFROST_SYNC_LOGIN_BROWSER_DRY_RUN_FILE`，验证可达远端首次启动只记录 1 条 `/v4/sso/logout?next=...` 登录 URL，`sync-state.json` 写入 `startup_login_prompt`，同一数据目录重启后登录 URL 仍只有 1 条；不可达本地端口场景等待三次短间隔启动预检后没有记录登录 URL。
- 2026-06-02：CI 回归 `test_sync_login_direct_e2e.sh` 发现启动预检重试等待会延迟 token 登录授权；修复后执行 `bash e2e-tests/tests/test_sync_login_direct_e2e.sh` 通过，验证 token 登录后 `/api/sync/status` 及时返回 `authorized:true` 和 `user_id:"ci-user"`。

---

### TC-ASN-34：启动登录预检 — 调试环境变量禁用自动弹窗

**操作步骤**：
1. 执行隔离 E2E 脚本：
   ```bash
   bash e2e-tests/tests/test_sync_startup_login_preflight_e2e.sh
   ```
2. 脚本中的 `environment disables startup login prompt` 场景会启动本地 mock sync server，并使用临时数据目录写入启用状态的 Sync 配置：
   ```toml
   [sync]
   enabled = true
   auto_sync = true
   remote_base_url = "http://127.0.0.1:<mock_sync_port>"
   probe_interval_secs = 2
   connect_timeout_ms = 500
   ```
3. 启动 Bifrost 时额外设置：
   ```bash
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
   ```
4. 同时设置 `BIFROST_SYNC_LOGIN_BROWSER_DRY_RUN_FILE`，用文件记录本来要打开的登录 URL，避免真实弹浏览器。
5. 等待管理端 ready 后继续等待 1 秒，检查 dry-run 文件中 `/v4/sso/logout?next=...` 登录 URL 数量。
6. 如果生成了 `sync-state.json`，检查其中没有写入 `startup_login_prompt`。

**预期结果**：
- 即使 Sync 默认启用、远端可达且本地无 token，设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 后也不会自动打开登录页。
- 禁用自动登录引导不会写入 `startup_login_prompt`，因此不会把“已经自动弹过”状态错误持久化。
- 该环境变量只影响启动自动弹窗；手动 `bifrost sync login` 和 Admin API 空 body 登录仍可主动打开浏览器。
- 未登录状态下后台 tick 不会继续探测远端并反复弹窗。

**真实执行记录**：
- 2026-06-02：执行 `bash e2e-tests/tests/test_sync_startup_login_preflight_e2e.sh` 通过。脚本真实启动当前源码构建的 Bifrost，新增 `environment disables startup login prompt` 场景设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、临时 `BIFROST_DATA_DIR`、`--no-system-proxy` 和 `BIFROST_SYNC_LOGIN_BROWSER_DRY_RUN_FILE`，确认远端可达且无 token 时 dry-run 登录 URL 数量仍为 0，并且没有持久化 `startup_login_prompt`。
- 2026-06-03：执行 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-sync --lib -- --test-threads=1` 通过，确认外层禁用自动登录弹窗环境变量不会污染登录预检单测；需要验证自动弹窗语义的用例会在 env lock 内临时 unset 该变量，仅 dry-run 记录登录 URL，不真实打开浏览器。

---

### TC-ASN-35：CLI 直登缺少 token 值时提示 token 获取链接

**操作步骤**：
1. 使用临时数据目录、非 9900 端口和本地 mock sync server 启动 Bifrost，或直接执行隔离 E2E 脚本：
   ```bash
   bash e2e-tests/tests/test_sync_login_direct_e2e.sh
   ```
2. 脚本会执行默认 Provider 场景：
   ```bash
   BIFROST_DATA_DIR="<same_temp_dir>" target/release/bifrost -p <admin_port> sync login --token
   ```
3. 脚本会执行显式自定义 relay URL 场景：
   ```bash
   BIFROST_DATA_DIR="<same_temp_dir>" target/release/bifrost -p <admin_port> sync login --token --url "http://127.0.0.1:<mock_sync_port>"
   ```
4. 检查两条命令的退出码和 stderr/stdout。

**预期结果**：
- 两条命令都在真正发起登录前失败，退出码为 1。
- 默认 Provider 场景输出包含 `--token must not be empty`。
- 默认 Provider 场景输出包含 `Sync session token for non-interactive login; get one at https://bifrost.bytedance.net/v4/sso/token-login`。
- 自定义 relay URL 场景输出包含 `--token must not be empty`。
- 自定义 relay URL 场景输出包含 `Sync session token for non-interactive login; get one at http://127.0.0.1:<mock_sync_port>/v4/sso/token-login`。
- 命令不会打开浏览器、不会写入新的 sync session token，也不会修改系统代理。

**真实执行记录**：
- 2026-06-17：执行 `bash e2e-tests/tests/test_sync_login_direct_e2e.sh` 通过。脚本先从当前 checkout 重新构建 `target/release/bifrost`，再使用临时 `BIFROST_DATA_DIR`、随机 Admin 端口、本地 mock sync server 和 `--no-system-proxy` 启动隔离服务；`bifrost sync login --token` 返回退出码 1，输出包含 `--token must not be empty` 和默认链接 `https://bifrost.bytedance.net/v4/sso/token-login`；`bifrost sync login --token --url http://127.0.0.1:<mock_sync_port>` 同样返回退出码 1，并输出 mock relay 链接 `http://127.0.0.1:<mock_sync_port>/v4/sso/token-login`。后续 token-only、token+URL、API token-only、API URL-only 和默认 Provider 回归均通过，成功输出使用当前文案 `Login successful`, 未打开浏览器，未修改系统代理。

---

### TC-ASN-36：一级 `bifrost login` 与 `bifrost sync login` 等价

**操作步骤**：
1. 使用临时数据目录、非 9900 端口和本地 mock sync server 启动 Bifrost，或直接执行隔离 E2E 脚本：
   ```bash
   bash e2e-tests/tests/test_sync_login_direct_e2e.sh
   ```
2. 查询一级登录命令帮助：
   ```bash
   target/release/bifrost login --help
   ```
3. 执行一级登录命令缺少 token 值的默认 Provider 场景：
   ```bash
   BIFROST_DATA_DIR="<same_temp_dir>" target/release/bifrost -p <admin_port> login --token
   ```
4. 执行一级登录命令缺少 token 值的自定义 relay URL 场景：
   ```bash
   BIFROST_DATA_DIR="<same_temp_dir>" target/release/bifrost -p <admin_port> login --token --url "http://127.0.0.1:<mock_sync_port>"
   ```
5. 执行一级登录命令显式 token+URL 场景：
   ```bash
   BIFROST_DATA_DIR="<same_temp_dir>" target/release/bifrost -p <admin_port> login --token ci-token --url "http://127.0.0.1:<mock_sync_port>"
   ```

**预期结果**：
- `bifrost login --help` 输出包含 `Equivalent to \`bifrost sync login\``、`--token`、`--url` 和默认 token 获取链接 `https://bifrost.bytedance.net/v4/sso/token-login`。
- `bifrost login --token` 在真正发起登录前失败，退出码为 1，输出包含 `--token must not be empty` 和默认 token 获取链接。
- `bifrost login --token --url http://127.0.0.1:<mock_sync_port>` 在真正发起登录前失败，退出码为 1，输出包含 `--token must not be empty` 和 mock relay token 获取链接。
- `bifrost login --token ci-token --url http://127.0.0.1:<mock_sync_port>` 与 `bifrost sync login --token ci-token --url ...` 一样完成直登，输出 `Login successful`。
- 所有场景均不打开浏览器、不会修改系统代理。

**真实执行记录**：
- 2026-06-17：执行 `bash e2e-tests/tests/test_sync_login_direct_e2e.sh` 通过。脚本先从当前 checkout 重新构建 `target/release/bifrost`，再使用临时 `BIFROST_DATA_DIR`、随机 Admin 端口、本地 mock sync server 和 `--no-system-proxy` 启动隔离服务；`bifrost login --help` 输出包含 `Equivalent to \`bifrost sync login\`` 和默认 token 获取链接；`bifrost login --token` 返回退出码 1，输出包含 `--token must not be empty` 和默认链接 `https://bifrost.bytedance.net/v4/sso/token-login`；`bifrost login --token --url http://127.0.0.1:<mock_sync_port>` 返回退出码 1，并输出 mock relay 链接 `http://127.0.0.1:<mock_sync_port>/v4/sso/token-login`；`bifrost login --token ci-token --url http://127.0.0.1:<mock_sync_port>` 输出 `Login successful`，与 `bifrost sync login` 等价。全流程未打开浏览器，未修改系统代理。
- 2026-07-07：执行 `SKIP_BUILD=true BIFROST_BIN=$PWD/target/release/bifrost BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 bash e2e-tests/tests/test_sync_login_direct_e2e.sh` 通过。确认当前 CLI 成功文案为 `Login successful`; token-only 默认 Provider 登录后通过 `/api/sync/status` 验证 `remote_base_url` 仍为 `https://bifrost.bytedance.net`, 不再依赖 CLI 输出旧版 `Remote URL: ...` 文案。

---

## 清理

测试完成后清理临时数据：
```bash
rm -rf .bifrost-test
```
