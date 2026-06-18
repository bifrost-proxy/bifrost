# auth-status 真实场景测试

> 模块：JWT/Cookie 登录态诊断
> 对应代码：`crates/bifrost-admin/src/auth_inspect.rs`、
> `crates/bifrost-admin/src/handlers/traffic.rs`（auth-status 分支）、
> `crates/bifrost-cli/src/commands/traffic.rs::run_traffic_auth_status`。

执行前置：

1. 已通过 `bifrost start` 启动本地代理。
2. 通过浏览器或脚本捕获若干请求，确保能在 `bifrost traffic list` 拿到 ID。

## TC-AS-01 valid JWT + valid Cookie

**目标**：构造一个携带未过期 JWT 与未过期 Cookie 的请求，CLI 显示双 valid，
overall 为 `valid for ...`。

**步骤**：

1. 构造请求脚本（curl 也可）：
   ```bash
   JWT=$(python3 - <<'PY'
   import base64, json, time
   p = {"exp": int(time.time()) + 3600, "sub": "user-42"}
   def b(x): return base64.urlsafe_b64encode(x).rstrip(b'=').decode()
   print(f"{b(b'{\"alg\":\"HS256\"}')}.{b(json.dumps(p).encode())}.sig")
   PY)
   curl -x http://127.0.0.1:9900 \
     -H "Authorization: Bearer $JWT" \
     -H "Cookie: session=abc; Max-Age=7200" \
     https://httpbin.org/get
   ```
2. `bifrost traffic list --limit 5` 取最新 record ID。
3. `bifrost traffic auth-status <ID>`。

**预期**：

- jwt：present，user_id=user-42，valid。
- cookie：present，valid。
- overall：`valid for 59m...`（或接近的剩余时间）。

## TC-AS-02 已过期 JWT

**目标**：JWT exp 已过，CLI 显示 EXPIRED。

**步骤**：

1. 构造 exp=1 的 JWT，发送请求。
2. `bifrost traffic auth-status <ID> --format json` 检查 `valid:false`，`jwt_exp_ms:1000`。
3. `bifrost traffic auth-status <ID>` 检查 human 输出含 `EXPIRED`。

**预期**：

- `valid:false`，overall=EXPIRED。

## TC-AS-03 无任何认证 header

**目标**：请求中没有 Authorization / Cookie / X-*-Token。

**步骤**：

1. 通过代理发送 `curl -x http://127.0.0.1:9900 https://example.com/`。
2. `bifrost traffic auth-status <ID> --format json`。

**预期**：

- `has_jwt:false`, `has_cookie:false`, `valid:null`。
- human 输出：`jwt: absent`, `cookie: absent`, `overall: unknown (no expiry info captured)`。

## TC-AS-04 headers 截断 / record 不存在

**目标**：headers 数据缺失或 record 不存在时不 5xx。

**步骤**：

1. 取一条不存在的 ID：`bifrost traffic auth-status not-an-id`。
   - 预期：非 0 退出，错误消息 `Traffic record 'not-an-id' not found`。
2. 直接命中 HTTP API：
   `curl -i http://127.0.0.1:9900/_bifrost/api/traffic/no-such-id/auth-status`
   - 预期：HTTP 404，body 含 `Traffic record 'no-such-id' not found`。
3. 对一条没有任何捕获 headers 的 record（比如 CONNECT tunnel）执行
   auth-status，验证返回 `has_jwt:false, has_cookie:false, valid:null`，HTTP 200。

**预期**：

- 任何情况都没有 500。

---

执行结果记录：测试人 / 日期 / 实际输出 / 通过/失败。
