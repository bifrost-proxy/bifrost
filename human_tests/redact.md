# Redact 层人工测试用例（`bifrost traffic get`）

> 对应设计文档：`design/redact.md`。

## 准备

```bash
# 启动 bifrost
bifrost start

# 选一条带 Authorization / Cookie 的最近请求 ID（例如登录请求）
TRAFFIC_ID=$(bifrost traffic list -f json | jq -r '.records[0].id')
echo "TRAFFIC_ID=$TRAFFIC_ID"
```

## TC-RD-01：默认 redact（必须通过）

```bash
bifrost traffic get "$TRAFFIC_ID" --request-body --response-body
```

**预期**：

- `Authorization` 头展示为 `<REDACTED:len=N>`。
- 任意 `Cookie` / `Set-Cookie` / `X-Api-Key` / `X-Tt-Passport-*` 头同样被替换。
- `Content-Type: application/json` 的请求/响应体中，`password`、`token`、
  `access_key`、`refreshToken` 等字段值替换为 `"<REDACTED:len=N>"`。
- 非敏感字段（`Content-Type`、`Accept`、业务 `user.name`）保持原样。

## TC-RD-02：`--show-secrets` 关闭 redact（必须通过）

```bash
bifrost traffic get "$TRAFFIC_ID" --request-body --response-body --show-secrets
```

**预期**：

- 所有 header 及 body 显示原值。
- CLI help 中此 flag 的说明明确警告"will print raw secrets"。

## TC-RD-03：`--extract-auth-summary` 输出摘要（必须通过）

```bash
bifrost traffic get "$TRAFFIC_ID" --extract-auth-summary --format json-pretty
```

**预期**：

- JSON 输出仍然 redact。
- 顶层多出 `auth_summary` 对象，包含：
  - `has_jwt` / `has_cookie` 布尔
  - `cookie_names` 数组（仅 cookie 名字，不含 value）
  - 若是 JWT，`jwt_user_id`（来自 `sub` / `user_id` / `uid`）和
    `jwt_exp_unix`
  - `host`

## TC-RD-04：`BIFROST_REDACT=off` 应急关闭（必须通过）

```bash
BIFROST_REDACT=off bifrost traffic get "$TRAFFIC_ID" --request-body --response-body
```

**预期**：

- 等价于 `--show-secrets`，所有 header / body 显示原值。
- 设置 `BIFROST_REDACT=on`（或不设置）后行为恢复 redact。

## 回归确认

- `bifrost search <kw> --format json` 默认行为同样 redact 命中行（preview）中
  出现的 token 字面量；P0-3 合入 `--include` 后由该 wave 补充 body redact。
- `bifrost replay` 使用 raw 数据，不受本层影响（设计文档说明）。
