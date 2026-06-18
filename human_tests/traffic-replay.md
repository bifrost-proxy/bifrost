# 人工测试：traffic export & replay (P2-6)

前置：本机 `bifrost start` 运行中，已有若干捕获流量；记下一个目标 record 的 sequence 后缀（如 `42`）和一个 host（如 `bits.bytedance.net`）。

## 用例 1：export curl
```
bifrost traffic export 42 --as curl
```
期望：标准输出一段 `curl -X ...` 命令；`Authorization` / `Cookie` / `X-*-Token` 行的值为 `<REDACTED>`；body 中 JSON 字段 `password` / `token` / `secret` / `jwt` / `sk-` 值为 `<REDACTED>`。

变体：`--show-secrets`：值不脱敏。`-o /tmp/req.sh`：写到文件，stderr 输出 `wrote /tmp/req.sh`。

## 用例 2：export HAR
```
bifrost traffic export 42 --as har | jq '.log.entries[0].request.method'
```
期望：输出 `"POST"`（或对应 method）；`jq '.log.version'` == `"1.2"`；`jq '.log.entries[0].request.headers[] | select(.name|ascii_downcase=="authorization").value'` == `"<REDACTED>"`。

## 用例 3：replay 改 body
对一个 JSON body 的请求：
```
bifrost traffic replay 42 --patch '/limit=5' --patch '/extras/-+="x"' --format json-pretty
```
期望：response `success: true`；`data.status` 是上游真实状态码；`data.duration_ms > 0`；服务器接收的 body 中 `limit=5`，`extras` 末尾追加 `"x"`。

错误用例：`--patch 'no-slash=1'` 应返回 CLI 解析错误 `path must start with '/'`。

## 用例 4：replay 带 refresh-auth
确保历史里至少存在一条同 host 的请求带 `Authorization`：
```
bifrost traffic replay 42 --refresh-auth --timeout 10s --format json-pretty
```
期望：`data.auth_refresh.applied == true`；`data.auth_refresh.source_traffic_id` 是另一个 record 的 id（不是 42）；`data.auth_refresh.fields` 包含 `Authorization`（或 `Cookie` / `X-Tt-*`）。

负向：若历史里同 host 无认证 header，`applied == false`，`source_traffic_id == null`。

## 用例 5（兼容回归）：旧字段 refresh_auth_from
直接走 HTTP：
```
curl -s -XPOST http://127.0.0.1:9000/_bifrost/api/traffic/<id>/replay \
  -H 'Content-Type: application/json' \
  -d '{"refresh_auth_from":"latest"}'
```
期望：行为等价 `refresh_auth=true`。