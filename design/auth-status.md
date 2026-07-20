# Auth Status: JWT / Cookie 登录态诊断

## 背景

调试登录失败、token 过期、单点登录链路问题时，用户需要快速判断某条捕获请求里到底带没带登录态、token 什么时候过期、user_id 是什么。手动 base64 解 JWT payload、读 `Set-Cookie` 的 `Expires` 和 `Max-Age` 是一件高频、低价值、易出错的重复劳动；每次登录问题排查都要重复一次。

Bifrost 已经持有完整的请求头与响应头（`TrafficRecord.request.headers` / `response.headers`），也已经落库到 `TrafficDbStore`。既然数据都在，就应该由 Bifrost 原地给出登录态摘要，让用户在 CLI 或 WebUI 一步看到「有没有 JWT / 有没有 Cookie / 现在还有效吗 / user_id 是什么 / 什么时候过期」，避免把 token 明文暴露到剪贴板或第三方工具里。

本设计描述 `crates/bifrost-admin/src/auth_inspect.rs` 与 `bifrost traffic auth-status` CLI 的完整语义、数据结构、识别规则、隐私边界、测试计划和风险收敛，作为 JWT/Cookie 诊断能力的权威文档。

## 用户目标验证清单

### 必须实现

- 对每条 traffic 记录能返回 `AuthSummary`，字段包含 host、has_jwt、has_cookie、jwt_exp_ms、jwt_user_id、cookie_exp_ms、valid_at_ms、valid。
- JWT 识别覆盖 `Authorization: Bearer <token>`、`X-Jwt-Token`、`X-Auth-Token`、`jwt-Token`（不区分大小写），以及 `Cookie: <name>=<value>` 里 name 含 `jwt` / `token` 的候选项。
- JWT 解析容忍 URL-safe base64 无 padding，多候选 exp 取最早，`sub | user_id | uid` 顺序回退，数字 user_id 自动转字符串。
- Cookie 过期解析同时支持 `Expires=<HTTP-date>` 与 `Max-Age=<sec>`；同一条 Set-Cookie 里两者同时出现取较早者；多条 Set-Cookie 也取最早。
- `valid` 判定：任一 exp ≤ now → `Some(false)`；所有 exp > now → `Some(true)`；全无 exp → `None`。
- 提供 HTTP API `GET /_bifrost/api/traffic/{id}/auth-status` 与 CLI `bifrost traffic auth-status <ID> [--format human|json]`。
- 输出永不包含原始 token 字符串或原始 Set-Cookie；只回显 user_id、过期时间和 valid 状态。
- 解析任意阶段失败必须安全 fallback，不 panic、不返回 500。

### 必须不破坏

- `TrafficDbStore` 的读写接口、traffic list/get/search、devtools capture、replay 都不受本能力影响。
- 请求处理路径不引入额外阻塞：`auth-status` 只在 CLI/HTTP 调用时按需解析，不写库、不做后台常驻。
- 已存在的 `AuthSummary` 序列化字段（snake_case）不做重命名，避免打断 IM 卡片或 WebUI 后续消费。
- 隐私策略：即使 body_store 已经保留完整 token，`auth-status` 也不允许把 token 明文回显到任何 API/CLI 输出。

### 必须真实验证

- 单元测试覆盖 JWT / Cookie / valid 判定的边界，且能在 workspace test 稳定通过。
- CLI 真实调用：`bifrost traffic auth-status <id>` 分别以 `human` 与 `json` 输出验证有 JWT / 有 Cookie / 过期 / 无认证 / headers 已截断 5 类形态。
- human_tests `human_tests/auth-status.md` 4 用例真实执行并归档。

## 产品语义

### auth-status 是只读诊断

`auth-status` 是「照相机」：给一条已经捕获的请求，报告它当时的登录态。它不会改写、注入、代理、刷新 token；也不会做服务端会话校验。它服务两个人：

- 排障工程师：想快速看到「用户当时是不是过期了」；
- 安全审计人：想验证某个客户端有没有正确清理 token、有没有跨域漏带 Set-Cookie。

### 只做客户端可见推断

`auth-status` 不会：

- 校验 JWT 签名。因为它不持有 HMAC secret 或 RSA public key。
- 解密加密 token（例如 JWE）。
- 查询 IdP 会话服务端状态。
- 猜测服务端 session（例如 PHPSESSID）是否还有效——只能报告 cookie 的过期时间。

`auth-status` 只做：URL-safe base64 解 payload、读 exp、读 sub/user_id/uid、解 Set-Cookie 的 Expires/Max-Age，然后综合判断当前时刻是否所有 exp 都还未到。

### 隐私

- API 响应结构里不存在 `token` 字段，也没有 raw header 字段。
- CLI human 输出只有 `user_id=<sub>`、`exp=<ISO ts>`、`valid|EXPIRED|no-exp|session|session-only`。
- CLI json 输出直接序列化 `AuthSummary`；同样不含 token。
- 解析失败也不会把出错 token 片段打印到 stderr；只在 debug 日志里 `trace!`。

## 数据模型

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthSummary {
    pub host: String,
    pub has_jwt: bool,
    pub has_cookie: bool,
    pub jwt_exp_ms: Option<i64>,
    pub jwt_user_id: Option<String>,
    pub cookie_exp_ms: Option<i64>,
    pub valid_at_ms: Option<i64>,
    pub valid: Option<bool>,
}
```

字段说明：

- `host`：从请求 URL 或 `Host` header 取，空字符串保留但 CLI 显示为 `(unknown)`。
- `has_jwt`：从任一 JWT 来源识别到候选 token 即为 true；即使 payload 解不动也保持 true，避免用户以为「没识别到 token 就等于没 JWT」。
- `has_cookie`：请求头里存在 `Cookie` 或响应头里存在 `Set-Cookie` 即为 true。
- `jwt_exp_ms`：payload `exp`（秒）× 1000；多候选取最早。
- `jwt_user_id`：payload `sub` 优先，回退 `user_id`、`uid`；数字自动 `to_string()`。
- `cookie_exp_ms`：Set-Cookie 中 `Expires`/`Max-Age` 综合最早过期时间。
- `valid_at_ms`：解析时 `now`，方便前端算「还剩多久」。
- `valid`：`None` 表示无 exp 信息，不冒然判断。

## 识别规则

### JWT 来源

1. 请求头 `Authorization: Bearer <token>`：strip `Bearer ` 前缀再 trim。
2. 请求头 `X-Jwt-Token`、`X-Auth-Token`、`jwt-Token`（header key 大小写不敏感），直接取 value trim。
3. 请求头 `Cookie: a=1; b=2`：按 `;` 分割、逐个 `k=v`；若 name lowercase 包含 `jwt` 或 `token` 子串，把 value 收入候选池。

多个候选都尝试 `parse_jwt_exp`，一旦解出 `exp` 就更新 `jwt_exp_ms` 为 `min(current, new)`；`jwt_user_id` 仅在为 `None` 时首次赋值，避免多 token 覆盖。

### JWT 解析（不验签）

```rust
pub fn parse_jwt_exp(token: &str) -> Option<(i64, Option<String>)> {
    let mut parts = token.trim().split('.');
    let _header = parts.next()?;
    let payload_b64 = parts.next()?;
    let _signature = parts.next(); // 允许缺
    let payload = decode_b64_url(payload_b64)?;
    let value: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    let exp = value.get("exp").and_then(|v| v.as_i64())?;
    let user_id = value.get("sub").or_else(|| value.get("user_id")).or_else(|| value.get("uid"))
        .and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        });
    Some((exp.checked_mul(1000)?, user_id))
}
```

- base64 解码走 `URL_SAFE_NO_PAD`，失败再 trim `=` 后重试；两次都不行返回 `None`。
- payload 不是合法 JSON、或没有 `exp`、或 `exp * 1000` 溢出 i64 → `None`。
- 三段 token 没有 signature 段仍尝试解 payload（容错）。
- **不做签名校验。** 因为我们没有 secret；即使有，也不在本模块职责内。
- 看起来像 JWT（多段、含 `.`）但解不动时，上层仍设 `has_jwt = true`、`jwt_exp_ms = None`，让用户知道有 token、只是解析失败。

### Cookie 过期解析

`parse_cookie_expiry_at(set_cookie, now_ms)`：

- 按 `;` 切分属性，找 `Expires=<value>` 与 `Max-Age=<seconds>`（前缀大小写不敏感）。
- `Expires`：用 `chrono::DateTime::parse_from_rfc2822` 优先解析，回退到多个 HTTP-date 格式（`%a, %d %b %Y %H:%M:%S GMT`、`%A, %d-%b-%y %H:%M:%S GMT` 等 4 种）。
- `Max-Age`：解析为 i64 秒，`now_ms + max_age * 1000`。
- 两者都有取 `min`；只有 `Max-Age` 且 `now_ms = None` 则 `Max-Age` 忽略；两者都无返回 `None`（session cookie）。

请求头 `Cookie:` 与响应头 `Set-Cookie:` 均遍历。多条 Set-Cookie 逐条解析，`cookie_exp_ms` 取所有已解出过期时间的最小值。

### valid 判定

```text
if all exp are None: valid = None
elif any exp <= now: valid = Some(false)
else: valid = Some(true)
```

其中 exp 集合 = `{jwt_exp_ms, cookie_exp_ms}` 中不为 None 的部分。

## HTTP API

`GET /_bifrost/api/traffic/{id}/auth-status`

- 200 + JSON `AuthSummary`：正常路径。
- 404：record 不存在。
- 200 + `{has_*: false, valid: null}`：body_store 已经 truncate 掉 headers 的极端场景，仍返回 200，不抛 500。这样调用方脚本能一致处理「没有 headers 可解」的场景。

handler 位置：`crates/bifrost-admin/src/handlers/traffic.rs::get_traffic_auth_status`；路由分发在同文件 `handle_traffic` 中根据 `rest.strip_suffix("/auth-status")` 判定，method 限 GET。

## CLI

```
bifrost traffic auth-status <ID> [--format human|json|json-pretty]
```

- CLI 定义：`crates/bifrost-cli/src/cli.rs::TrafficCommands::AuthStatus { port, id, format }`。
- Runner：`crates/bifrost-cli/src/commands/traffic.rs::run_traffic_auth_status` 通过 `direct_agent` 打到 `http://127.0.0.1:<port>/_bifrost/api/traffic/{id}/auth-status`。
- 404 → 返回 `BifrostError::NotFound("Traffic record '<id>' not found")`。
- 网络失败 → `BifrostError::Network`。
- JSON 解析失败 → `BifrostError::Parse`。
- OutputFormat：
  - `Json` / `JsonPretty` / `Ndjson`：直接序列化 `AuthSummaryView`。
  - `Table` / `Compact`：走 `print_auth_summary_human`（默认 human）。

human 示例：

```text
host: api.example.com
jwt: present (user_id=12345, exp=2026-06-17T20:00:00Z, valid)
cookie: present (exp=2026-06-18T00:00:00Z, valid)
overall: valid for 1h23m
```

字段渲染：

- `jwt: absent` / `jwt: present (...)`；`exp=unknown, no-exp` 表示识别到 JWT 但解析不出 exp。
- `cookie: absent` / `cookie: present (exp=..., valid|EXPIRED|session)`；`exp=session-only, session` 表示无过期信息。
- `overall: valid for <duration>` / `overall: EXPIRED` / `overall: unknown (no expiry info captured)`。
- duration 由 `format_duration_short` 计算，形如 `2d`、`1h23m`、`45s`、`0s`。

## Admin API 契约

`AuthSummary` 序列化字段命名不能变（`host`、`has_jwt`、`has_cookie`、`jwt_exp_ms`、`jwt_user_id`、`cookie_exp_ms`、`valid_at_ms`、`valid`）；`traffic.rs::auth_status_tests` 已有一个 handler 单测断言字段命名与往返序列化，防止意外重命名破坏 API 契约。

未来若要新增字段（例如 `bearer_scheme`、`multi_jwt_count`），必须只加不改，保持向后兼容。

## Sync 边界

`auth-status` 是本地只读诊断，不参与 Sync：

- `AuthSummary` 不落库；每次调用现算。
- 不进入 rule sync、group sync、rule share。
- 远端 Bifrost 通过 `bifrost remote traffic get <id>` 拉到 record 后，调用方本地跑一次 `auth_inspect` 即可。若要新增 `bifrost remote traffic auth-status`，也应该复用 admin HTTP endpoint，不新增独立 sync channel。

## 实现切分

### Phase 1：核心解析模块

- `crates/bifrost-admin/src/auth_inspect.rs`：`AuthSummary`、`parse_jwt_exp`、`parse_cookie_expiry_at`、`build_auth_summary(headers, host, now_ms)`。
- 单元测试 17 条覆盖 JWT / Cookie / 边界。
- `#[cfg(test)]` block 直接放在同文件末尾，无额外集成测试目录。

### Phase 2：HTTP handler + 单测

- `crates/bifrost-admin/src/handlers/traffic.rs::get_traffic_auth_status`：加载 record → 组装 headers → `build_auth_summary` → JSON 响应。
- 路由分发：`rest.strip_suffix("/auth-status")`，限 GET。
- handler 单测 1 条：`AuthSummary` 字段命名与往返序列化。

### Phase 3：CLI + 单测

- CLI subcommand `AuthStatus { port, id, format }`。
- `run_traffic_auth_status` → HTTP → `AuthSummaryView` → 输出格式化。
- `print_auth_summary_human`、`format_ts_iso`、`format_duration_short`。
- CLI 单测 2 条：`format_duration_short` 数值边界；`print_auth_summary_human` 不 panic。

### Phase 4：human_tests 与文档

- `human_tests/auth-status.md` 4 用例（valid、expired JWT、无认证、headers 截断），逐条真实跑。
- 更新本设计文档。
- README/CLI help 补充 `bifrost traffic auth-status` 用法。

## 测试方案

### 单元测试（17 条 in auth_inspect.rs）

- JWT exp 解析、sub 解析、user_id / uid 回退、数字 user_id 转字符串。
- URL-safe base64 无 padding 与带 padding 兼容。
- 非法 token（少段、payload 非 JSON、exp 非 i64、exp * 1000 溢出）返回 None。
- 看起来像 JWT 但解析失败仍上层标 `has_jwt = true, exp = None`。
- Cookie `Expires` 单独存在、`Max-Age` 单独存在、两者同时存在取较早、多条 Set-Cookie 取最早、session cookie。
- 空 headers 返回全 false + valid = None。
- Bearer 前缀识别、Cookie 中 name 含 `jwt`/`token` 的候选提取。
- 过期 JWT → valid = Some(false)；未过期 JWT → valid = Some(true)。

### handler 单测（1 条 in traffic.rs::auth_status_tests）

- `AuthSummary` 字段命名与 serde 往返：构造实例 → `to_value` → 字段 key 断言 → `from_value` → 断言等价。

### CLI 单测（2 条 in traffic.rs::auth_status_tests）

- `format_duration_short_handles_hours_and_minutes`：0s / 45s / 1h23m / 2d。
- `print_auth_summary_handles_all_states`：valid / expired / no-exp / session / absent 组合下不 panic 且输出包含预期关键词。

### E2E 测试

- 复用 admin HTTP 起停脚手架，新增 `test_traffic_auth_status_api.sh`（若尚未存在）：
  - 启动临时 Bifrost。
  - 通过 Rules 注入一次带 `Authorization: Bearer <sample_jwt>` 的响应。
  - `curl http://127.0.0.1:<port>/_bifrost/api/traffic/<id>/auth-status | jq` 断言 has_jwt、user_id、valid、exp。
  - 断言 404 场景对未知 id 返回 `Traffic record '...' not found`。

### 真实场景测试 human_tests

`human_tests/auth-status.md` 4 用例：

- TC-AUTH-01：valid JWT + valid Cookie，human 输出显示 `overall: valid for ...`。
- TC-AUTH-02：expired JWT，`overall: EXPIRED`、`jwt: present (..., EXPIRED)`。
- TC-AUTH-03：无 JWT 无 Cookie，`jwt: absent / cookie: absent / overall: unknown`。
- TC-AUTH-04：headers 已被 body_store truncate，返回 has_*: false + valid: null，CLI 显示 unknown。

所有 human_tests 使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo test -p bifrost-admin auth_inspect -- --nocapture`
- `cargo test -p bifrost-admin auth_status_tests`
- `cargo test -p bifrost-cli auth_status_tests`
- `cargo test --workspace --all-features`
- `rust-project-validate`

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：JWT/Cookie 识别、过期解析、valid 判定、CLI 输出、隐私。
- 复核 diff：`auth_inspect.rs` / `traffic.rs handler` / `traffic.rs CLI` / human_tests 是否闭环。
- 重点 review：
  - JWT payload 解析失败仍标 has_jwt = true。
  - Cookie 多来源（请求 Cookie + 响应 Set-Cookie）都被覆盖。
  - CLI 与 HTTP 输出永不出现 token 明文（grep 源码 `token`、`Bearer`、`Set-Cookie` 使用点）。
- 复测：单元 17 + handler 1 + CLI 2；`cargo test --workspace --all-features`。

### 第 2 轮

- 复核第 1 轮问题修复。
- 再看 `git status --short` / `git diff`，确认没漏改 human_tests 索引。
- 重点 review：
  - 陌生 HTTP-date 格式回落 4 种是否覆盖真实浏览器/服务端常见写法。
  - `valid_at_ms` 由 handler 注入 `Utc::now().timestamp_millis()`，避免测试时钟不一致。
  - `AuthSummary` 序列化字段与前端约定一致。
- 复测：human_tests TC-AUTH-01..04 真实跑；`bifrost traffic auth-status <id> --format json` 输出结构核对。

## 风险与决策

- chrono RFC1123/HTTP-date 解析在极端浏览器/服务端写法下仍可能失配；当前回落 4 种格式，超出时返回 `None`，调用方仍能看到 `has_cookie = true`。可接受，后续按真实 case 逐步扩展。
- body_store truncate headers 时（极端容量压力），summary 不带 token 信息，valid = null；产品明确接受，不上升为 500。
- JWE / 加密 token 不做解析：这是设计上的边界，避免让 Bifrost 变成密钥托管系统。
- 后续可能新增 WebUI 「⚠ 已过期」徽标、「user 切换」对比视图、`bifrost traffic list --auth-expired` 过滤器；这些都基于本模块的 AuthSummary，属于展示层扩展，不改本模块契约。
- 不做告警：`auth-status` 是被动查询工具，若要主动告警（例如「登录态即将过期」），需要走独立后台任务，不侵入本模块。
