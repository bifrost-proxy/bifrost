# Auth Status: JWT / Cookie 登录态诊断

## 背景

调试登录失败、token 过期、单点登录链路问题时，需要快速判断捕获的请求中
是否携带了登录态、token 是否过期、user_id 是什么。手动 base64 解 JWT、
读 Set-Cookie 的 Expires 是一个高重复劳动。Bifrost 已经持有完整请求/响应
头，可以原地给出这个诊断。

## 范围

为每条 traffic 记录提供 **HTTP API** 与 **CLI** 形式的「登录态摘要」：

- 是否携带 JWT；如有，过期时间和 user_id；
- 是否携带 Cookie；如有，最近一个 Set-Cookie 的过期时间；
- 综合判断当前时刻是否「仍有效」。

非目标（不在本期）：

- 服务端会话校验、解码加密 token；
- 自动告警 / WebUI 卡片（后续 wave）；
- 重写、注入、自动刷新 token。

## 数据模型

```rust
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

- 字段说明
  - `has_jwt`：从 Authorization/X-Jwt-Token/X-Auth-Token/jwt-Cookie 任意来源识别到 JWT。
  - `jwt_exp_ms`：JWT payload 的 `exp`（秒）×1000；若多个候选，取**最早**。
  - `jwt_user_id`：优先 `sub`，回退 `user_id` / `uid`；数字会转字符串。
  - `cookie_exp_ms`：Set-Cookie 中的 Expires/Max-Age 综合后的最早过期时间。
  - `valid`：仅当至少有一个 exp 时返回 true/false；都没有时为 `None`。

## 识别规则

### JWT 来源
- `Authorization: Bearer <token>`
- `X-Jwt-Token`, `X-Auth-Token`, `jwt-Token`（不区分大小写）
- `Cookie: <name>=<value>` 中 cookie 名包含 `jwt`/`token` 的子串。

### JWT 解析
- 三段 `header.payload.signature`，中间段做 URL-safe base64 解码（允许无 padding）。
- `serde_json::from_slice` 拿 payload；读取 `exp`（秒）、`sub|user_id|uid`。
- 失败/缺字段一律返回 None。**不会验证签名。**
- 看起来像 JWT（多段、含 `.`）但 payload 解不动时，仍记 `has_jwt=true`、不设 exp。

### Cookie 过期
- 解析每个 `Set-Cookie`：
  - `Expires=<RFC1123 date>` 用 chrono 的 `parse_from_rfc2822` + 多个回退格式。
  - `Max-Age=<seconds>` 基于 `valid_at_ms` 转 epoch ms。
  - 同时出现 → 取**较早**者。
- 多个 Set-Cookie → 取**最早**过期时间（最保守）。

### valid 判定
- 任一 exp ≤ now → `valid=Some(false)`；
- 所有 exp 都 > now → `valid=Some(true)`；
- 全无 exp → `valid=None`。

## HTTP API

```
GET /_bifrost/api/traffic/{id}/auth-status
```

- 200 → `AuthSummary` JSON。
- 404 → record 不存在。
- headers 整体丢失（store 已 truncate）→ 200 返回 `has_*=false, valid=null`，**不抛 500**。

## CLI

```
bifrost traffic auth-status <ID> [--format human|json]
```

- `--format human`（默认）：人类可读多行输出；
- `--format json`：序列化 AuthSummary。

human 示例：
```
host: bits.bytedance.net
jwt: present (user_id=12345, exp=2026-06-17T20:00:00Z, valid)
cookie: present (exp=2026-06-18T00:00:00Z, valid)
overall: valid for 1h23m
```

## 隐私与安全

- **永远不输出原始 token 字符串。** API 返回字段里没有 token；CLI 也仅
  打印 user_id / 过期时间 / valid。
- payload 解析仅取 `exp/sub/user_id/uid`；其余字段忽略。
- 不做签名校验，也不打印 header；HMAC secret 不在视野内。
- 解析任何阶段失败都安全 fallback 到 `has_*=true, exp=None`，避免泄漏 panic
  栈或部分 token。

## 实现位置

- 模块：`crates/bifrost-admin/src/auth_inspect.rs`。
- HTTP 处理：`crates/bifrost-admin/src/handlers/traffic.rs::get_traffic_auth_status`。
- CLI：`crates/bifrost-cli/src/commands/traffic.rs::run_traffic_auth_status`，
  `crates/bifrost-cli/src/cli.rs::TrafficCommands::AuthStatus`。

## 测试

- 单测 17：JWT exp / sub / uid / 数字 user_id / 带 padding 的 base64 /
  非法 token / Cookie Expires / Max-Age / Expires+Max-Age 同时存在取较早 /
  会话 cookie / 空 headers / Bearer / Cookie 中 jwt name / 过期 JWT /
  Set-Cookie 多条取最早 / 看起来像 JWT 但解析失败仍标 has_jwt。
- handler 单测 1：AuthSummary 字段命名与往返序列化。
- CLI 单测 2：format_duration_short + print_auth_summary_human 不 panic。
- human_tests：`human_tests/auth-status.md` 4 用例（valid、expired JWT、无认证、headers 截断）。

## 风险与 TODO

- chrono RFC1123 解析在不同浏览器/服务端写法下覆盖有限；当前回退 4 种格式，
  超出时会返回 `None`，调用方仍能看到 `has_cookie=true`。
- 当 Bifrost body_store 已 truncate headers（极端容量压力），返回的 summary
  将不带任何 token 信息，valid=null。这是预期行为。
- 后续可在 WebUI 增加「⚠ 已过期」徽标和「user 切换」对比视图，留给下一波。
