# 代理认证暴力破解防护测试

## 功能模块说明

验证 HTTP 代理和 SOCKS5 代理的用户名/密码认证暴力破解防护机制（SEC-05），包括：
- 每 IP 失败计数追踪
- 达到 10 次失败后临时封禁 5 分钟
- HTTP 代理返回 429 Too Many Requests
- SOCKS5 代理断开连接
- 认证成功后计数重置
- 封禁期过后自动解除（自动清理机制）

## 前置条件

1. 启动 Bifrost 服务：

```bash
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl
```

2. 通过 API 配置代理认证并启用 loopback 认证要求（`--proxy-user` 启动参数会硬编码 `loopback_requires_auth=false`，所以必须通过 API 配置）：

```bash
curl -s -X PUT http://127.0.0.1:8800/_bifrost/api/whitelist/userpass \
  -H "Content-Type: application/json" \
  -d '{"enabled":true,"accounts":[{"username":"testuser","password":"TestPass123","enabled":true}],"loopback_requires_auth":true}'
```

3. 验证代理认证已生效：

```bash
curl -s -o /dev/null -w "%{http_code}" -x http://127.0.0.1:8800 http://httpbin.org/get
# 预期：407（需要代理认证）
```

> **注意**：每个涉及封禁的测试用例执行前需要重启服务以重置 rate limiter 计数器。重启后配置会自动从持久化存储恢复。

## 测试用例列表

### TC-PAB-01: HTTP 代理 — 正确凭证可正常通过

**操作步骤：**
1. 使用正确的代理凭证发起请求：
   ```bash
   curl -s -o /dev/null -w "%{http_code}" \
     -x http://testuser:TestPass123@127.0.0.1:8800 \
     http://httpbin.org/get
   ```

**预期结果：**
- 返回 HTTP 200

### TC-PAB-02: HTTP 代理 — 错误凭证返回 407

**操作步骤：**
1. 使用错误的代理凭证发起请求：
   ```bash
   curl -s -o /dev/null -w "%{http_code}" \
     -x http://testuser:WrongPass@127.0.0.1:8800 \
     http://httpbin.org/get
   ```

**预期结果：**
- 返回 HTTP 407

### TC-PAB-03: HTTP 代理 — 连续失败不超过阈值仍可正常认证

**操作步骤：**
1. 连续发送 5 次错误凭证请求：
   ```bash
   for i in $(seq 1 5); do
     echo "--- Attempt $i ---"
     curl -s -o /dev/null -w "HTTP %{http_code}\n" \
       -x http://testuser:WrongPass@127.0.0.1:8800 \
       http://httpbin.org/get
   done
   ```
2. 然后使用正确凭证请求：
   ```bash
   curl -s -o /dev/null -w "%{http_code}" \
     -x http://testuser:TestPass123@127.0.0.1:8800 \
     http://httpbin.org/get
   ```

**预期结果：**
- 5 次错误请求均返回 HTTP 407
- 正确凭证请求返回 HTTP 200（未达 10 次阈值，未被封禁）

### TC-PAB-04: HTTP 代理 — 达到 10 次失败后返回 429

**操作步骤：**
1. 先重启服务以重置计数器（或等待清理周期后执行）：
   ```bash
   # 先停止前一个服务，重新启动（配置已持久化，无需再通过 API 配置）
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl
   ```
2. 连续发送 10 次错误凭证请求：
   ```bash
   for i in $(seq 1 10); do
     echo "--- Attempt $i ---"
     curl -s -o /dev/null -w "HTTP %{http_code}\n" \
       -x http://testuser:WrongPass@127.0.0.1:8800 \
       http://httpbin.org/get
   done
   ```
3. 发送第 11 次请求（无论凭证正确与否）：
   ```bash
   curl -s -w "\nHTTP %{http_code}\n" \
     -x http://testuser:TestPass123@127.0.0.1:8800 \
     http://httpbin.org/get
   ```

**预期结果：**
- 前 10 次错误请求返回 HTTP 407
- 第 11 次请求返回 HTTP 429
- 响应头包含 `Retry-After: 300`
- 响应体包含 "Too many failed authentication attempts"

### TC-PAB-05: HTTP 代理 — 封禁期间即使正确凭证也被拒绝（429）

**操作步骤：**
1. 紧接 TC-PAB-04 之后（IP 已被封禁），使用正确凭证发起请求：
   ```bash
   curl -s -w "\nHTTP %{http_code}\n" \
     -x http://testuser:TestPass123@127.0.0.1:8800 \
     http://httpbin.org/get
   ```

**预期结果：**
- 返回 HTTP 429（IP 被封禁，直接拒绝，不进入凭证验证）

### TC-PAB-06: SOCKS5 代理 — 正确凭证可正常通过

**操作步骤：**
1. 使用正确的 SOCKS5 凭证发起请求：
   ```bash
   curl -s -o /dev/null -w "%{http_code}" \
     --socks5 127.0.0.1:8800 \
     --proxy-user testuser:TestPass123 \
     http://httpbin.org/get
   ```

**预期结果：**
- 返回 HTTP 200

### TC-PAB-07: SOCKS5 代理 — 错误凭证返回连接错误

**操作步骤：**
1. 使用错误的 SOCKS5 凭证发起请求：
   ```bash
   curl -s -o /dev/null -w "%{http_code}" \
     --socks5 127.0.0.1:8800 \
     --proxy-user testuser:WrongPass \
     http://httpbin.org/get 2>&1
   ```

**预期结果：**
- curl 返回连接错误（SOCKS5 认证失败断开连接）
- 不返回 HTTP 200

### TC-PAB-08: SOCKS5 代理 — 达到 10 次失败后封禁

**操作步骤：**
1. 先重启服务重置计数器
2. 连续发送 10 次错误 SOCKS5 凭证：
   ```bash
   for i in $(seq 1 10); do
     echo "--- Attempt $i ---"
     curl -s --socks5 127.0.0.1:8800 \
       --proxy-user testuser:WrongPass \
       http://httpbin.org/get 2>&1 | head -1
   done
   ```
3. 第 11 次用正确凭证：
   ```bash
   curl -s --socks5 127.0.0.1:8800 \
     --proxy-user testuser:TestPass123 \
     http://httpbin.org/get 2>&1
   ```

**预期结果：**
- 前 10 次认证失败（连接错误）
- 第 11 次即使凭证正确也被拒绝（IP 被封禁）

### TC-PAB-09: 认证成功后失败计数重置

**操作步骤：**
1. 重启服务重置计数器
2. 发送 5 次错误凭证：
   ```bash
   for i in $(seq 1 5); do
     curl -s -o /dev/null -w "HTTP %{http_code}\n" \
       -x http://testuser:WrongPass@127.0.0.1:8800 \
       http://httpbin.org/get
   done
   ```
3. 发送 1 次正确凭证：
   ```bash
   curl -s -o /dev/null -w "HTTP %{http_code}\n" \
     -x http://testuser:TestPass123@127.0.0.1:8800 \
     http://httpbin.org/get
   ```
4. 再发送 9 次错误凭证（不应触发封禁，因为计数已重置）：
   ```bash
   for i in $(seq 1 9); do
     echo "--- Attempt $i ---"
     curl -s -o /dev/null -w "HTTP %{http_code}\n" \
       -x http://testuser:WrongPass@127.0.0.1:8800 \
       http://httpbin.org/get
   done
   ```
5. 发送 1 次正确凭证：
   ```bash
   curl -s -o /dev/null -w "HTTP %{http_code}\n" \
     -x http://testuser:TestPass123@127.0.0.1:8800 \
     http://httpbin.org/get
   ```

**预期结果：**
- 步骤 2：5 次返回 407
- 步骤 3：返回 200
- 步骤 4：9 次返回 407（不是 429，因为成功认证后计数已重置）
- 步骤 5：返回 200

### TC-PAB-10: 不同 IP 独立计数（非 loopback 验证）

**操作步骤：**
1. 此用例验证 ProxyAuthRateLimiter 的单元测试中的 IP 隔离逻辑
2. 使用 cargo test 验证：
   ```bash
   cargo test -p bifrost-core test_rate_limiter
   ```

**预期结果：**
- 单元测试通过，验证不同 IP 的失败计数互不影响

### TC-PAB-11: Account CLI 多账号管理、真实代理流量与加密落盘

**操作步骤：**
1. 使用隔离数据目录执行 userpass loopback E2E：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d)" BIFROST_DISABLE_TRAY=1 \
     BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_userpass_loopback_e2e.sh
   ```
2. 观察脚本中的 `Account CLI multi-account CRUD, real proxy traffic, and encrypted config` 场景。
3. 场景内部会执行：
   ```bash
   printf 'alpha-secret-123\n' | bifrost -p <PORT> account add alice --password-stdin --enable-auth
   printf 'beta-secret-123\n' | bifrost -p <PORT> account add bob --password-stdin
   printf 'disabled-secret-123\n' | bifrost -p <PORT> account add disabled-user --password-stdin --disabled
   printf 'bifrost-local-secret:not-json\n' | bifrost -p <PORT> account add prefix-user --password-stdin
   bifrost -p <PORT> account list --json
   bifrost -p <PORT> account set-loopback-auth true
   printf 'beta-secret-456\n' | bifrost -p <PORT> account update bob --password-stdin
   bifrost -p <PORT> account update bob --disable
   bifrost -p <PORT> account update bob --enable
   bifrost -p <PORT> account remove alice
   ```
4. 场景会通过 Bifrost HTTP 代理访问本地临时 upstream，验证无凭证、正确凭证、错误密码、禁用账号、删除账号的真实代理行为。
5. 场景会检查 `BIFROST_DATA_DIR/config.toml` 中不包含任何账号密码明文，包含 `bifrost-local-secret:` envelope，并生成 Unix `0600` 的 `local_config_secret.key`。
6. 场景会停止 target、移除 `USER` / `USERNAME` / `USERPROFILE` 后用同一 data dir 重启，再验证 `prefix-user:bifrost-local-secret:not-json` 仍能通过真实代理请求。

**预期结果：**
- `account add --enable-auth` 后 Admin API `.userpass.enabled` 为 `true`，四个账号均可见且 `has_password=true`。
- `account set-loopback-auth true` 后本机无凭证代理请求返回 407，使用 `alice:alpha-secret-123` 和 `bob:beta-secret-123` 可通过本地 upstream 真实代理请求。
- 错误密码、禁用账号、删除账号均返回 407。
- `account update bob --password-stdin` 后旧密码返回 407，新密码返回 200。
- `account update bob --disable/--enable` 后代理认证行为随账号状态变化。
- `config.toml` 只保存 `bifrost-local-secret:` 加密 envelope，不保存明文账号密码。
- 同一 data dir 在缺少 USER 系列环境变量的启动上下文中仍能解密；以 `bifrost-local-secret:` 开头的合法密码重启后不被误解析为 envelope。

### TC-PAB-12: HTTP 代理真实流量防暴力破解与成功重置

**操作步骤：**
1. 使用隔离数据目录执行 userpass loopback E2E：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d)" BIFROST_DISABLE_TRAY=1 \
     BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_userpass_loopback_e2e.sh
   ```
2. 观察脚本中的 `HTTP Proxy brute-force limit and success reset` 场景。
3. 场景内部会先用正确凭证重置计数，连续 5 次错误密码后再用正确凭证确认可通过。
4. 场景随后连续 9 次错误密码并再次用正确凭证确认未触发封禁。
5. 场景最后连续 10 次错误密码，再用正确凭证请求，确认被封禁。

**预期结果：**
- 错误密码在阈值前返回 407。
- 成功认证会重置失败计数。
- 达到阈值后的下一次请求返回 `429 Too Many Requests`。
- 429 响应包含 `Retry-After: 300` 和 `Too many failed authentication attempts`。

### TC-PAB-13: Access Control 直达加载与全局开关禁用后账号持久化回归

**操作步骤：**
1. 执行真实 Chromium UI 回归：
   ```bash
   pnpm --dir web exec playwright test tests/ui/admin-settings.spec.ts \
     --grep '重新挂载和保存后保留 CLI 账号与认证开关'
   ```
2. 用例先通过 Admin API 模拟 CLI 已创建一个启用账号，并设置全局认证和 localhost 认证为启用。
3. 浏览器直接打开 `http://127.0.0.1:<UI_PORT>/_bifrost/settings?tab=access`，不点击其他 Settings tab，检查账号和两个开关；在 light/dark theme 下各检查一次。
4. 离开 Settings 后从侧边栏返回 Access Control，关闭全局认证并保存；通过 API 检查账号、`has_password` 和 localhost 策略。
5. 刷新直达 URL，重新检查账号和两个开关；再次启用全局认证并保存。
6. 执行隔离数据目录的 CLI 与重启回归：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d)" BIFROST_DISABLE_TRAY=1 \
     BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_userpass_loopback_e2e.sh
   ```
7. 观察 `Account CLI multi-account CRUD, real proxy traffic, and encrypted config` 场景中的 `account disable`、`account list --json`、`account enable` 和同一 data dir 重启断言。

**预期结果：**
- 直达 `?tab=access` 后无需切换 tab，已保存账号、全局开关和 localhost 开关立即显示正确。
- light/dark theme 切换不会丢失或隐藏账号数据。
- 全局认证关闭并保存后，API 与刷新后的 WebUI 仍保留账号、`has_password=true`、账号自身 enabled 状态和 localhost 策略。
- 再次启用全局认证时无需重输密码，原账号继续存在。
- CLI `account disable` 只改变 `.enabled=false`；`account list --json` 仍返回全部账号，随后 `account enable` 和服务重启后账号及加密密码仍可用。

## 清理步骤

```bash
# 停止 Bifrost 服务（Ctrl+C）
rm -rf ./.bifrost-test
```

## 执行记录

### 2026-07-10 Account CLI 与加密落盘回归

| 用例 | 结果 | 证据 |
| --- | --- | --- |
| TC-PAB-11 | PASS | 2026-07-10 修复 review 问题后执行 `BIFROST_DATA_DIR="$(mktemp -d)" BIFROST_DISABLE_TRAY=1 BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_userpass_loopback_e2e.sh`，输出 `Tests Run: 12`、`Tests Passed: 12`、`Tests Failed: 0`。除多账号 CRUD 与真实 HTTP 代理 200/407 外，新增 `prefix-user` 密码 `bifrost-local-secret:not-json`，确认 `config.toml` 不含该明文、`local_config_secret.key` 已生成；随后停止 target、移除 USER 系列变量后用同一 data dir 重启，prefix-user 仍真实认证返回 200。 |
| TC-PAB-12 | PASS | 同一次 E2E 中 `HTTP Proxy brute-force limit and success reset` PASS，验证 5 次错误后正确凭证重置计数、9 次错误后正确凭证仍通过、10 次错误后的下一次请求返回 `429 Too Many Requests`，且响应包含 `Retry-After: 300` 与 `Too many failed authentication attempts`。 |
| TC-PAB-13 | PASS | 2026-07-10 执行真实 Chromium Playwright 用例，初次 human test 输出 `1 passed (20.1s)`，第 1 轮修复后复跑输出 `1 passed (1.6m)`；直达 `?tab=access` 无需切 tab 即显示已有账号和两个开关，light/dark、离开再返回、global disable 保存、reload、再次 enable 全部通过。随后以隔离 data dir 重跑 userpass shell E2E，输出 `Tests Run: 12`、`Tests Passed: 12`、`Tests Failed: 0`，确认 CLI disable/list/enable 和重启均保留四个账号及加密密码。 |
