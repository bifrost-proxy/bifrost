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
   bifrost -p <PORT> account list --json
   bifrost -p <PORT> account set-loopback-auth true
   printf 'beta-secret-456\n' | bifrost -p <PORT> account update bob --password-stdin
   bifrost -p <PORT> account update bob --disable
   bifrost -p <PORT> account update bob --enable
   bifrost -p <PORT> account remove alice
   ```
4. 场景会通过 Bifrost HTTP 代理访问本地临时 upstream，验证无凭证、正确凭证、错误密码、禁用账号、删除账号的真实代理行为。
5. 场景会检查 `BIFROST_DATA_DIR/config.toml` 中不包含 `alpha-secret-123`、`beta-secret-123`、`beta-secret-456` 或 `disabled-secret-123` 明文，并包含 `bifrost-local-secret:` envelope。

**预期结果：**
- `account add --enable-auth` 后 Admin API `.userpass.enabled` 为 `true`，三个账号均可见且 `has_password=true`。
- `account set-loopback-auth true` 后本机无凭证代理请求返回 407，使用 `alice:alpha-secret-123` 和 `bob:beta-secret-123` 可通过本地 upstream 真实代理请求。
- 错误密码、禁用账号、删除账号均返回 407。
- `account update bob --password-stdin` 后旧密码返回 407，新密码返回 200。
- `account update bob --disable/--enable` 后代理认证行为随账号状态变化。
- `config.toml` 只保存 `bifrost-local-secret:` 加密 envelope，不保存明文账号密码。

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

## 清理步骤

```bash
# 停止 Bifrost 服务（Ctrl+C）
rm -rf ./.bifrost-test
```

## 执行记录

### 2026-07-10 Account CLI 与加密落盘回归

| 用例 | 结果 | 证据 |
| --- | --- | --- |
| TC-PAB-11 | PASS | 执行 `BIFROST_DATA_DIR="$(mktemp -d)" BIFROST_DISABLE_TRAY=1 BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_userpass_loopback_e2e.sh`，输出 `Tests Run: 12`、`Tests Passed: 12`、`Tests Failed: 0`。其中 `Account CLI multi-account CRUD, real proxy traffic, and encrypted config` PASS，验证 `account add/list/update/remove/set-loopback-auth` 多账号管理、正确账号真实 HTTP 代理请求返回 200、错误密码/禁用账号/删除账号返回 407，并确认 `config.toml` 不包含账号密码明文且包含 `bifrost-local-secret:` envelope。 |
| TC-PAB-12 | PASS | 同一次 E2E 中 `HTTP Proxy brute-force limit and success reset` PASS，验证 5 次错误后正确凭证重置计数、9 次错误后正确凭证仍通过、10 次错误后的下一次请求返回 `429 Too Many Requests`，且响应包含 `Retry-After: 300` 与 `Too many failed authentication attempts`。 |
